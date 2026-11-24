//! **Cancelling a call** (identity-and-lifecycle I4, PR 3): the cancel-forward, the cancel receipt, and the door's
//! handle on a call it dispatched.
//!
//! # Who promises what
//!
//! A cancel crosses three agents, and each answers only for itself:
//!
//! - **The gateway** promises what it did: it revoked the call's term — signed the revoking checkpoint and applied it to
//!   its own authority ([`call_issuer::begin_revoke`](super::call_issuer::begin_revoke)) — **forwarded it to the
//!   provider at once**, and recorded it on disk ([`call_issuer::finish_revoke`](super::call_issuer::finish_revoke)).
//!   The revocation is its own signed act, true when signed, so the forward does not wait for the disk; only the
//!   acknowledgement does (§9 O10, the maintainer's decision of 2026-11-20). A [`CancelReceipt`] exists only once the
//!   revocation is on disk; a revoke the disk refused is [`CancelRefusal::NotDurable`] — **the forward was still
//!   sent**, so the call may have been stopped, and the caller is told the stop is held in memory only.
//! - **The provider** promises its own stop. It holds the revocation (so the term is refused if the call reaches it
//!   later), asks the work registered under the term to stop (its sweep), and answers what it saw: a stop is
//!   [`CancelClass::Stopped`] only where the agent doing the work promised to stop — the door drops the handler's
//!   future, or an `rpc_rx` receiver declared it races the lapse — and that work has ended. Work handed to an agent
//!   that made no such promise is [`CancelClass::Unbounded`]: asked to stop, its end not the provider's to confirm.
//! - **The receipt names its rung and nothing above it** (the receipts ADR, `docs/design/contracts-receipts.md`):
//!   no answer in time is [`CancelClass::DeliveryUnknown`], never a negative — the revocation is recorded and
//!   reaches the provider by the KV as well — and a gateway that does not know where the call went (it restarted:
//!   D-4 keeps nothing) says [`CancelClass::NotForwarded`].
//!
//! # What a caller of `cancel_call` is answered (§9 O10)
//!
//! The forward and the durable write run together; the answer waits for both.
//!
//! | The durable write | The forward | The answer |
//! |---|---|---|
//! | on disk | answered | `Ok(CancelReceipt)`, the provider's class |
//! | on disk | no answer in [`CANCEL_FORWARD_TIMEOUT_MS`], or not sent (no target known) | `Ok(CancelReceipt)`, `DeliveryUnknown` or `NotForwarded` — the KV carries it |
//! | refused | answered, or not | `Err(CancelRefusal::NotDurableForwarded { class, .. })`: the provider's class beside the refusal (#821's review, F2); call again to write it (idempotent, the forward sent again) |
//! | the gateway crashes before it returns | sent, perhaps answered | **no answer at all**: the revocation is in no KV after the restart, and the provider holds it in memory only (D-4) — after a provider restart the call, re-presented inside its window, is admitted |
//!
//! A door ([`cancel_at_door`]) forwards on a task of its own and answers once the durable write returns: the door's
//! own refusal on `NotDurable` (A2A `-32603`; MCP's notification has no answer and logs it).
//!
//! # The cancel-forward
//!
//! The primary cancel path (the maintainer's decision on #775's review, H1): the gateway sends the revoking
//! checkpoint **to the provider the call went to**, as an RPC of kind [`CANCEL_RPC_KIND`], so a member that rewrites
//! the `call/…/checkpoint` key in the KV cannot deny it. The KV copy stays the fallback, and the record a restarted
//! or later-admitting provider reads.
//!
//! The kind is the substrate's (`sys.`): the gateway's raw routes refuse it for every token, sent, served or observed
//! (`SUBSTRATE_KINDS`). It is **not** a protected kind — the provider's preflight (`ae_decide`) and the exchanged
//! token do not apply to a gateway's cancel (#766's M5). The provider accepts it only **from the term's issuing
//! gateway**: the request must come in an envelope its sender signed as itself, and that sender must be the
//! checkpoint's authority; otherwise it is refused `cancel_not_from_issuer` and nothing moves.
//!
//! # The doors
//!
//! A door that dispatches a call holds a [`CallSink`] for it ([`CALL_SINK`], a task-local the dispatch reads): the
//! dispatch fills in the term it minted, and the door's cancel reads it. A cancel that arrives before the mint sets
//! the sink's flag, and the dispatch — which checks it once its records are on disk — revokes the term and does not
//! send the call (`cancelled_before_sent`). MCP `notifications/cancelled` finds the sink in a bounded table keyed by
//! `(principal, MCP session, request id)`; A2A `tasks/cancel` finds it on the task.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};

use super::gateway_authority::CallRevocationHeld;
use super::gateway_caller::node_principal;
use super::TaskCtx;
use crate::mandate::authority::SignedRevocationCheckpoint;
use crate::mandate::PrincipalId;
use crate::node_id::NodeId;

/// The cancel-forward's RPC kind: the substrate's, refused at every gateway door.
pub(crate) const CANCEL_RPC_KIND: &str = "sys.call-cancel.v1";

/// How long a provider waits, after it asked the work under a term to stop, for that work to end before it answers
/// [`CancelClass::StopRequested`] instead of [`CancelClass::Stopped`]: the grace a declared receiver has to answer
/// after a lapse ([`LAPSE_ANSWER_GRACE_MS`](super::provider_enforcement::LAPSE_ANSWER_GRACE_MS)).
pub const CANCEL_CONFIRM_WAIT_MS: u64 = super::provider_enforcement::LAPSE_ANSWER_GRACE_MS;

/// How long the gateway waits for the provider's answer to a cancel-forward before the receipt says
/// [`CancelClass::DeliveryUnknown`]: the provider's wait and room for the round trip. Each attempt waits as long for its
/// own answer.
pub const CANCEL_FORWARD_TIMEOUT_MS: u64 = 5_000;

/// **How soon an unanswered cancel-forward is sent again** while its receipt waits (§9 O13, the maintainer's decision
/// of 2026-11-22): every 500 ms, so four attempts leave inside *F* = 2 s (bound 1) and ten inside
/// [`CANCEL_FORWARD_TIMEOUT_MS`]. Short enough that a send lost on a full outbound queue is replaced well inside bound
/// 1; long enough that a provider answering in the ordinary tens of milliseconds is never sent a second.
pub const CANCEL_FORWARD_RETRY_MS: u64 = 500;

/// The longest gap between two attempts. Once the receipt has said [`CancelClass::DeliveryUnknown`] the gap doubles
/// from one second to this, the order of the KV's anti-entropy interval, which carries the same revocation.
pub const CANCEL_FORWARD_RETRY_CAP_MS: u64 = 30_000;

/// **The most attempts one cancel-forward makes**, the first among them: ten while the receipt waits, and fourteen
/// more on the back-off, which reach about 305 s — past the default 300 s call window. A forward stops sooner on an
/// answer, at the call's window, or at shutdown.
pub const MAX_CANCEL_FORWARD_ATTEMPTS: u32 = 24;

/// How many cancel-forwards a provider waits on at once. Past it, an answer is given at once, from what the sweep
/// left running ([`CancelClass::StopRequested`] where something still runs).
const MAX_CONFIRM_WAITS: usize = 64;

/// The most calls the gateway's `/mcp` door holds a cancel handle for at once (the plan's bounded id → term table).
pub const MAX_MCP_CANCEL_ENTRIES: usize = 65_536;

/// **A cancel's receipt** (identity-and-lifecycle I4; the maintainer's decision Q2 of 2026-11-12): the term the gateway
/// revoked, and what the provider answered, or why there is no answer. A receipt is returned only once the gateway
/// has recorded the revocation on disk — that is the rung below every class here. The forward is sent before that
/// rung is reached (§9 O10), so a class says what the provider answered whether or not the disk then took it; without
/// the disk there is no receipt (`CancelRefusal::NotDurable`).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelReceipt {
    /// The call's term, `c{n}`.
    pub term: String,
    /// What the provider answered for its own stop, or why it did not.
    pub class: CancelClass,
}

/// What a cancel reached, by rung — each the promise of the agent that answered, and nothing above it.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancelClass {
    /// The provider holds the revocation; work under the term was running there at a door whose agent promised to
    /// stop, the provider asked it to stop, and that work **has ended**, with its stop record. The rung is "the work
    /// promised to stop and has ended", not "the stop ended it": work that finished on its own just after the request
    /// is recorded the same way (a `WorkGuard` dropped after its cancel; #799's review, L4), so the requester may
    /// still have the call's result. At the SDK serve stream it is recorded only on the subscriber's own confirmation
    /// (`rpc/respond` `"stopped": true`, I4's PR 3b): the subscriber's word, never the node's inference.
    Stopped,
    /// The provider holds the revocation and asked the work under the term to stop; that work had not ended when it
    /// answered ([`CANCEL_CONFIRM_WAIT_MS`]). A stop asked for, not a stop seen.
    StopRequested,
    /// The provider holds the revocation and asked the work to stop, and the agent doing it made **no promise to
    /// stop** — a door that forwards or streams the call, a component that is never interrupted. The provider cannot
    /// confirm its end: the class's drain is unbounded (`DrainBound::Unbounded`), and the work runs at most to its
    /// mandate's window.
    Unbounded,
    /// The provider holds the revocation; a call under the term was admitted and registered there, nothing runs under
    /// it now, and no stop of it is recorded: it finished on its own. (A stop the revocation in the KV made a moment
    /// before the forward arrived is answered from its stop record — `Stopped`, or `Unbounded` for an unconfirmed
    /// release — while the provider keeps it, among its newest 1 024.)
    NotRunning,
    /// The provider has **not registered** a call under the term, so it cannot say whether one runs (#799's review,
    /// M1). The call has not reached it, or it runs there unregistered: the provider does not enforce (it registers
    /// no work and holds nothing), or a rule that is not cancellable admitted the call with its `call:` slot not
    /// established. Where the provider enforces it holds the revocation, so a call admitted there later under the
    /// term is refused `revoked`; one already running unregistered runs to its end.
    NotRegistered,
    /// The provider could not hold the revocation (its windows are at their cap); a call it admits later under the
    /// term reads the KV record.
    NotHeld,
    /// The provider refused the cancel-forward, by name: `cancel_not_from_issuer`, `malformed`, `not_verified`,
    /// `no_authority`, or a refusal at its receive boundary. The revocation is still recorded and in the KV.
    Refused(String),
    /// No answer within [`CANCEL_FORWARD_TIMEOUT_MS`] — **not a negative**: the forward or its answer may be lost, or
    /// the provider may predate the cancel-forward. The revocation reaches it by the KV, within bound 1.
    DeliveryUnknown,
    /// This gateway does not know which provider the call went to (it restarted since the mint: nothing is kept,
    /// D-4), so nothing was forwarded; the revocation reaches the provider by the KV.
    NotForwarded,
}

impl CancelClass {
    /// The stable name: the label of `GET /stats` `call_cancel_receipts` and of
    /// `mycelium_gateway_call_cancel_receipts_total`.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::StopRequested => "stop_requested",
            Self::Unbounded => "unbounded",
            Self::NotRunning => "not_running",
            Self::NotHeld => "not_held",
            Self::NotRegistered => "not_registered",
            Self::Refused(_) => "refused",
            Self::DeliveryUnknown => "delivery_unknown",
            Self::NotForwarded => "not_forwarded",
        }
    }

    fn from_label(label: &str, reason: Option<&str>) -> Option<Self> {
        Some(match label {
            "stopped" => Self::Stopped,
            "stop_requested" => Self::StopRequested,
            "unbounded" => Self::Unbounded,
            "not_running" => Self::NotRunning,
            "not_held" => Self::NotHeld,
            "not_registered" => Self::NotRegistered,
            "refused" => Self::Refused(reason.unwrap_or("refused").to_string()),
            _ => return None,
        })
    }
}

/// Why a cancel has no receipt. `NoIdentity` and `UnknownTerm`: nothing was revoked and nothing forwarded.
/// `NotDurable`: the revocation was signed and forwarded, and is not on disk.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancelRefusal {
    /// This node has no TLS identity to sign the revocation with.
    NoIdentity,
    /// This gateway holds no call of that term, in memory or as a verified window record of its own.
    UnknownTerm,
    /// The revoking checkpoint is not on disk. **It was still forwarded** (§9 O10, the maintainer's decision of
    /// 2026-11-20, reversing the rule that it *"stands in this process only, and is not forwarded"*): the provider the
    /// call went to holds it in memory and may have stopped the call — its answer is counted in
    /// `call_cancel_receipts` — and this gateway's own authority holds it. No restart of either keeps it, and the KV
    /// does not carry it. Called again, the cancel writes it (the same checkpoint) and forwards it again.
    NotDurable(String),
    /// **`cancel_call`'s** refusal when the revoking checkpoint is not on disk (#821's review, F2): as
    /// [`NotDurable`](Self::NotDurable) — forwarded, held in memory only, call again — and with what the provider
    /// answered the forward, so the caller learns whether its call stopped though the cancel is not durable. `class`
    /// is [`CancelClass::NotForwarded`] where this gateway did not know the provider (nothing was sent), and
    /// [`CancelClass::DeliveryUnknown`] where no answer came in time. The doors answer [`NotDurable`](Self::NotDurable):
    /// they forward on a task of their own and do not wait for its answer.
    NotDurableForwarded {
        /// What the provider answered the forward, or why there is no answer.
        class: CancelClass,
        /// Why the disk refused the revoking checkpoint.
        error: String,
    },
}

impl std::fmt::Display for CancelRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoIdentity => f.write_str("this gateway has no TLS identity to sign a cancel with"),
            Self::UnknownTerm => f.write_str("this gateway issued no call of that term"),
            Self::NotDurable(e) => write!(f, "the cancel could not be recorded on disk: it is held in memory only, here and at the provider it was forwarded to: {e}"),
            Self::NotDurableForwarded { class, error } => write!(
                f,
                "the cancel could not be recorded on disk: it is held in memory only, here and at the provider it was forwarded to, which answered {}: {error}",
                class.label()
            ),
        }
    }
}

impl std::error::Error for CancelRefusal {}

impl From<super::call_issuer::RevokeRefusal> for CancelRefusal {
    fn from(r: super::call_issuer::RevokeRefusal) -> Self {
        use super::call_issuer::RevokeRefusal as R;
        match r {
            R::NoIdentity => Self::NoIdentity,
            R::UnknownTerm => Self::UnknownTerm,
            R::NotDurable(e) => Self::NotDurable(e),
        }
    }
}

// ── the gateway's half ──────────────────────────────────────────────────────────────────────────────────────────

/// **Cancel the call of `term`**: revoke it, forward the revoking checkpoint to the provider the call went to **and**
/// record it on disk, the two together (§9 O10: the forward does not wait for the disk), and return what the provider
/// answered once both are done — `NotDurable` if the disk refused, the forward sent all the same (the module doc's
/// table). Called again, the revoke is idempotent and the checkpoint is forwarded again — joining the term's forward
/// if it still waits for its receipt (§9 O13, [`forward_once`]).
///
/// **The forward runs on a task of its own** (#821's review, F3), as the doors' does: a caller that drops this future
/// stops neither the forward nor its receipt's count. **Dropping it drops the durable write's wait**: a record the WAL's
/// writer already holds is still written and synced, but it is not applied to the KV or gossiped (the required-sync
/// write applies only once its sync returns to it), so until a restart replays the WAL — or the cancel is asked again,
/// which writes it once more — the revocation reaches the provider only by the forward, and is held there in memory
/// only.
pub(crate) async fn cancel(ctx: &Arc<TaskCtx>, term: &str) -> Result<CancelReceipt, CancelRefusal> {
    let pending = super::call_issuer::begin_revoke(ctx, term)?;
    let (tx, answered) = tokio::sync::oneshot::channel();
    let (ctx2, checkpoint, target, window_end_ms, term2) = (Arc::clone(ctx), pending.checkpoint.clone(), pending.target.clone(), pending.window_end_ms, term.to_string());
    ctx.spawn_task(async move {
        let settle = |class: CancelClass| {
            note_receipt(&ctx2, &class);
            let _ = tx.send(class);
        };
        match target {
            Some(target) => forward_once(&ctx2, &term2, target, &checkpoint, window_end_ms, settle).await,
            None => settle(CancelClass::NotForwarded),
        }
    });
    let durable = super::call_issuer::finish_revoke(ctx, &pending).await;
    // The forward settles its receipt by `CANCEL_FORWARD_TIMEOUT_MS` (its retries may run on past it); a task gone at
    // shutdown answered nothing.
    let class = answered.await.unwrap_or(CancelClass::DeliveryUnknown);
    match durable {
        Ok(_) => Ok(CancelReceipt { term: term.to_string(), class }),
        Err(super::call_issuer::RevokeRefusal::NotDurable(error)) => Err(CancelRefusal::NotDurableForwarded { class, error }),
        Err(other) => Err(other.into()),
    }
}

/// Count a receipt by its class.
fn note_receipt(ctx: &TaskCtx, class: &CancelClass) {
    let label = class.label();
    ctx.call_issuer.cancel_receipts.pin().update_or_insert(label, |n| n + 1, 1);
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_gateway_call_cancel_receipts_total", "class" => label).increment(1);
}

/// How long to wait before the attempt after the `sent`-th (`sent` ≥ 1): [`CANCEL_FORWARD_RETRY_MS`] while the receipt
/// waits, then doubling from one second to [`CANCEL_FORWARD_RETRY_CAP_MS`].
pub(crate) fn retry_delay_ms(sent: u32) -> u64 {
    let while_the_receipt_waits = (CANCEL_FORWARD_TIMEOUT_MS / CANCEL_FORWARD_RETRY_MS) as u32;
    if sent < while_the_receipt_waits {
        return CANCEL_FORWARD_RETRY_MS;
    }
    1_000u64.saturating_mul(2u64.saturating_pow(sent - while_the_receipt_waits)).min(CANCEL_FORWARD_RETRY_CAP_MS)
}

/// §9 O13: what this gateway's cancel-forwards did — `GET /stats` `call_cancel_forwards`, and
/// `mycelium_gateway_call_cancel_forward_attempts_total{attempt}` and `mycelium_gateway_call_cancel_forwards_total{outcome}`.
/// The evidence the maintainer's deferred option B (a lane of its own for control signals) is to be decided on.
#[derive(Default)]
pub(crate) struct ForwardStats {
    /// Every cancel-forward request sent, the first of each forward and its re-sends.
    pub(crate) attempts: AtomicU64,
    /// The re-sends among them.
    pub(crate) retries: AtomicU64,
    /// Forwards whose answer came on their first attempt.
    pub(crate) answered_first: AtomicU64,
    /// Forwards whose first answer came on a re-send, before the receipt was settled: the first attempt, or its answer,
    /// was lost or slower than the re-send's.
    pub(crate) answered_on_retry: AtomicU64,
    /// Forwards answered only after their receipt had said `delivery_unknown`: the provider holds the revocation, and
    /// the receipt given stands (a receipt is never revised).
    pub(crate) answered_after_receipt: AtomicU64,
    /// Forwards that ended with no answer at all: at their attempt bound, at the call's window, or at shutdown.
    pub(crate) unanswered: AtomicU64,
    /// Forwards, already settled and still re-sending unanswered, that a newer cancel of the same term replaced
    /// (#826's review, F2): at most one forward per term runs.
    pub(crate) superseded: AtomicU64,
    /// Cancels of a term whose forward was still waiting for its receipt: they joined it and sent nothing of their own
    /// (#826's review, F2).
    pub(crate) joined: AtomicU64,
    /// As a provider: repeats of a forward that joined the answer already in progress for the same term instead of
    /// waiting on their own (#826's review, F1).
    pub(crate) provider_joined: AtomicU64,
    /// The forward running for each term, by term.
    in_flight: papaya::HashMap<String, Arc<InFlight>>,
}

/// One term's running forward: its receipt's class once settled, and the signal that a newer cancel replaced it.
#[derive(Default)]
struct InFlight {
    class: OnceLock<CancelClass>,
    note: tokio::sync::Notify,
    /// `notify_one`: the permit is kept for the forward's loop if it is not waiting at that moment.
    superseded: tokio::sync::Notify,
}

impl InFlight {
    fn settled(&self, class: &CancelClass) {
        let _ = self.class.set(class.clone());
        self.note.notify_waiters();
    }

    /// Its forward ended without settling (a panic): whoever joined it is answered no answer, never a negative.
    fn abandon(&self) {
        if self.class.get().is_none() {
            self.settled(&CancelClass::DeliveryUnknown);
        }
    }

    /// The running forward's receipt class, waiting up to `within_ms` for it.
    async fn receipt(&self, within_ms: u64) -> Option<CancelClass> {
        let deadline = mycelium_core::sim_seam::mono_instant() + Duration::from_millis(within_ms);
        loop {
            let noted = self.note.notified();
            tokio::pin!(noted);
            noted.as_mut().enable();
            if let Some(class) = self.class.get() {
                return Some(class.clone());
            }
            let left = deadline.saturating_duration_since(mycelium_core::sim_seam::mono_instant());
            if left.is_zero() {
                return None;
            }
            tokio::select! {
                _ = noted => {}
                _ = mycelium_core::sim_seam::sleep_ms("authority/cancel-forward-joined", left.as_millis() as u64) => {}
            }
        }
    }
}

impl ForwardStats {
    fn note_attempt(&self, retry: bool) {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        if retry {
            self.retries.fetch_add(1, Ordering::Relaxed);
        }
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_gateway_call_cancel_forward_attempts_total", "attempt" => if retry { "retry" } else { "first" }).increment(1);
    }

    fn note_outcome(&self, outcome: &'static str) {
        let n = match outcome {
            "answered_first" => &self.answered_first,
            "answered_on_retry" => &self.answered_on_retry,
            "answered_after_receipt" => &self.answered_after_receipt,
            "superseded" => &self.superseded,
            _ => &self.unanswered,
        };
        n.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_gateway_call_cancel_forwards_total", "outcome" => outcome).increment(1);
    }

    /// The `/stats` object.
    pub(crate) fn to_json(&self) -> Value {
        let n = |a: &AtomicU64| a.load(Ordering::Relaxed);
        json!({
            "attempts": n(&self.attempts),
            "retries": n(&self.retries),
            "answered_first": n(&self.answered_first),
            "answered_on_retry": n(&self.answered_on_retry),
            "answered_after_receipt": n(&self.answered_after_receipt),
            "unanswered": n(&self.unanswered),
            "superseded": n(&self.superseded),
            "joined": n(&self.joined),
            "provider_joined": n(&self.provider_joined),
        })
    }
}

/// **One forward per term** (#826's review, F2): a cancel of a term whose forward is still waiting for its receipt
/// joins it — its answer is that receipt, and it sends nothing of its own; a cancel of a term whose forward has settled
/// and is still re-sending unanswered replaces it (the old one stops, `superseded`) and starts afresh, so a cancel asked
/// again is forwarded again, as before. So repeated door cancels and `cancel_call`s never run more than one forward, and
/// its at most ten live attempts, per term.
async fn forward_once(ctx: &Arc<TaskCtx>, term: &str, target: NodeId, checkpoint: &SignedRevocationCheckpoint, window_end_ms: u64, settle: impl FnOnce(CancelClass)) {
    let stats = &ctx.call_issuer.cancel_forwards;
    let mine = Arc::new(InFlight::default());
    // Bounded: each pass either takes the term, joins, or replaces a settled forward; a lost race goes round again.
    for _ in 0..8 {
        let current = match stats.in_flight.pin().try_insert(term.to_string(), Arc::clone(&mine)) {
            Ok(_) => None,
            Err(occupied) => Some(Arc::clone(occupied.current)),
        };
        let Some(current) = current else { break };
        if current.class.get().is_none() {
            stats.joined.fetch_add(1, Ordering::Relaxed);
            let class = current.receipt(CANCEL_FORWARD_TIMEOUT_MS + 1_000).await.unwrap_or(CancelClass::DeliveryUnknown);
            return settle(class);
        }
        // Settled, still re-sending: replace it, if it is still the one there (retry-safe: the closure decides on its
        // arguments alone).
        let replaced = matches!(
            stats.in_flight.pin().compute(term.to_string(), |entry| match entry {
                Some((_, cur)) if Arc::ptr_eq(cur, &current) => papaya::Operation::Insert(Arc::clone(&mine)),
                _ => papaya::Operation::Abort(()),
            }),
            papaya::Compute::Updated { .. }
        );
        if replaced {
            current.superseded.notify_one();
            break;
        }
    }
    // Removed however the forward ends — a panic in it included (#826's confirm round): an entry left unsettled would
    // have every later cancel of the term join it, wait 6 s and send nothing.
    let _entry = Entry { map: &stats.in_flight, key: term.to_string(), mine: Arc::clone(&mine), on_drop: InFlight::abandon };
    let wrapped = |class: CancelClass| {
        mine.settled(&class);
        settle(class);
    };
    forward(ctx, target, checkpoint, window_end_ms, &mine, wrapped).await;
}

/// **Send the revoking checkpoint to `target` until it answers** (§9 O13, the maintainer's decision of 2026-11-22: the
/// gateway keeps its promise that the provider hears of the cancel by its own effort). A self envelope carries each
/// attempt (`rpc_call_ctx`), which is what lets the provider know it came from the issuing gateway.
///
/// The cancel-forward is a best-effort Individual signal on the gateway's outbound queues, which drop a frame when they
/// are full; nothing tells the gateway a send was dropped. So an attempt is sent at once and again every
/// [`CANCEL_FORWARD_RETRY_MS`] while no answer has come, each waiting [`CANCEL_FORWARD_TIMEOUT_MS`] for its own
/// answer — the attempts overlap, so a provider still waiting for the work to end ([`CANCEL_CONFIRM_WAIT_MS`]) is not
/// cut off — and the first answer of any attempt ends the forward. A re-send is the same signed checkpoint, which the
/// provider holds idempotently and answers from what it holds.
///
/// `settle` is called **once**, with the receipt's class: the first answer, if one comes within
/// [`CANCEL_FORWARD_TIMEOUT_MS`] of the first attempt, else [`CancelClass::DeliveryUnknown`] — when the caller is
/// answered is as before this retry. Unanswered then, the forward goes on, on the back-off
/// ([`retry_delay_ms`]), so the provider still hears of the cancel before the KV's repair; an answer then is counted
/// (`answered_after_receipt`) and the receipt stands. No attempt is sent past [`MAX_CANCEL_FORWARD_ATTEMPTS`], once
/// `window_end_ms` has passed (the first excepted — the provider answers it from what it holds), or once the node is
/// shutting down; on shutdown the forward stops at once and its unanswered attempts are dropped.
async fn forward(ctx: &Arc<TaskCtx>, target: NodeId, checkpoint: &SignedRevocationCheckpoint, window_end_ms: u64, in_flight: &InFlight, settle: impl FnOnce(CancelClass)) {
    let Ok(payload) = serde_json::to_vec(&json!({ "v": 1, "checkpoint": checkpoint })) else { return settle(CancelClass::DeliveryUnknown) };
    let payload = Bytes::from(payload);
    // A test stands in for a forward never sent: the receipt says `DeliveryUnknown` at once, and only the KV carries it.
    #[cfg(test)]
    test_hooks::forward_gate(&ctx.node_id).await;
    #[cfg(test)]
    if test_hooks::forward_suppressed(&ctx.node_id) {
        return settle(CancelClass::DeliveryUnknown);
    }
    #[cfg(test)]
    if test_hooks::take_panic(&test_hooks::PANIC_IN_FORWARD, &ctx.node_id) {
        panic!("a test's planted panic in the cancel-forward");
    }
    let stats = &ctx.call_issuer.cancel_forwards;
    let mut shutdown = ctx.shutdown_tx.subscribe();
    let begun = mycelium_core::sim_seam::mono_instant();
    let receipt_at = begun + Duration::from_millis(CANCEL_FORWARD_TIMEOUT_MS);
    let mut settle = Some(settle);
    let mut attempts: tokio::task::JoinSet<(u32, Result<Bytes, super::rpc::RpcError>)> = tokio::task::JoinSet::new();
    let mut sent: u32 = 0;
    let mut next_at = Some(begun);
    let mut answered = false;
    let mut superseded = false;
    loop {
        if *shutdown.borrow() {
            break;
        }
        let now = mycelium_core::sim_seam::mono_instant();
        if let Some(at) = next_at
            && at <= now
        {
            if sent < MAX_CANCEL_FORWARD_ATTEMPTS && (sent == 0 || ctx.hlc.decision_now_ms() < window_end_ms) {
                let (ctx2, payload, target, i) = (Arc::clone(ctx), payload.clone(), target.clone(), sent);
                attempts.spawn(async move { (i, attempt(&ctx2, target, payload).await) });
                sent += 1;
                stats.note_attempt(sent > 1);
                next_at = (sent < MAX_CANCEL_FORWARD_ATTEMPTS).then(|| now + Duration::from_millis(retry_delay_ms(sent)));
            } else {
                next_at = None;
            }
        }
        if next_at.is_none() && attempts.is_empty() {
            break;
        }
        let wake = match (next_at, settle.is_some()) {
            (Some(at), true) => Some(at.min(receipt_at)),
            (Some(at), false) => Some(at),
            (None, true) => Some(receipt_at),
            (None, false) => None,
        };
        let sleep_ms = wake.map(|w| w.saturating_duration_since(now).as_millis() as u64);
        tokio::select! {
            biased;
            _ = shutdown.wait_for(|v| *v) => break,
            _ = in_flight.superseded.notified() => {
                superseded = true;
                break;
            }
            Some(done) = attempts.join_next(), if !attempts.is_empty() => {
                if let Ok((i, Ok(bytes))) = done {
                    let class = read_answer(&bytes);
                    match settle.take() {
                        Some(settle) => {
                            stats.note_outcome(if i == 0 { "answered_first" } else { "answered_on_retry" });
                            settle(class);
                        }
                        None => stats.note_outcome("answered_after_receipt"),
                    }
                    answered = true;
                    break;
                }
            }
            _ = mycelium_core::sim_seam::sleep_ms("authority/cancel-forward-retry", sleep_ms.unwrap_or(0)), if sleep_ms.is_some() => {}
        }
        if mycelium_core::sim_seam::mono_instant() >= receipt_at
            && let Some(settle) = settle.take()
        {
            settle(CancelClass::DeliveryUnknown);
        }
    }
    // Dropped with the set: no unanswered attempt outlives the forward.
    attempts.abort_all();
    if superseded {
        stats.note_outcome("superseded");
    } else if !answered {
        stats.note_outcome("unanswered");
    }
    if let Some(settle) = settle.take() {
        settle(CancelClass::DeliveryUnknown);
    }
}

/// One attempt of a cancel-forward: the request, and the provider's answer within [`CANCEL_FORWARD_TIMEOUT_MS`].
async fn attempt(ctx: &TaskCtx, target: NodeId, payload: Bytes) -> Result<Bytes, super::rpc::RpcError> {
    #[cfg(test)]
    {
        test_hooks::note_attempt(&ctx.node_id, ctx.hlc.decision_now_ms());
        // A test stands in for a send the gateway's outbound queue dropped: nothing reaches the provider, and the
        // attempt's answer never comes.
        if test_hooks::send_dropped(&ctx.node_id) {
            mycelium_core::sim_seam::sleep_ms("authority/cancel-forward-dropped", CANCEL_FORWARD_TIMEOUT_MS).await;
            return Err(super::rpc::RpcError::Timeout);
        }
    }
    // B1: the forward rides the control lane too, so it waits behind none of this node's gossip to the provider, nor
    // behind the provider's reader of that gossip (`docs/design/control-lane.md`).
    let reply = super::rpc::rpc_call_ctx_control(ctx, target, Arc::from(CANCEL_RPC_KIND), payload, Duration::from_millis(CANCEL_FORWARD_TIMEOUT_MS)).await;
    // A test stands in for a forward delivered whose answer is lost (#821's confirm round): the provider acted on it.
    #[cfg(test)]
    if reply.is_ok() && test_hooks::answer_lost(&ctx.node_id) {
        mycelium_core::sim_seam::sleep_ms("authority/cancel-forward-dropped", CANCEL_FORWARD_TIMEOUT_MS).await;
        return Err(super::rpc::RpcError::Timeout);
    }
    reply
}

/// A provider's answer as a class: its `class`, or — a refusal at its receive boundary, which names a `reason` —
/// `Refused`. Anything else was not an answer to this question.
fn read_answer(bytes: &[u8]) -> CancelClass {
    let Ok(v) = serde_json::from_slice::<Value>(bytes) else { return CancelClass::DeliveryUnknown };
    if let Some(label) = v.get("class").and_then(Value::as_str) {
        return CancelClass::from_label(label, v.get("reason").and_then(Value::as_str)).unwrap_or(CancelClass::DeliveryUnknown);
    }
    match v.get("reason").and_then(Value::as_str) {
        Some(reason) => CancelClass::Refused(reason.to_string()),
        None => CancelClass::DeliveryUnknown,
    }
}

/// **A door's handle on one call it dispatches.** The dispatch fills in the term once the call's records are on disk;
/// the door's cancel sets the flag and reads the term. Either order is caught: each side writes its half and then
/// reads the other's, behind a sequentially consistent fence, so at least one sees both.
#[derive(Default, Debug)]
pub(crate) struct CallSink {
    term: OnceLock<String>,
    cancelled: AtomicBool,
    /// What the provider answered the door's forward, once it has (#821's review, F1): the `/a2a` door reads it to
    /// tell the stop's own answer from a call that completed on its own.
    forwarded: OnceLock<CancelClass>,
    forwarded_note: tokio::sync::Notify,
    /// The `/a2a` door's send answered while its cancel was being recorded: parked here, not in the task table, for
    /// the cancel to decide (#821's review, F1).
    #[cfg(feature = "a2a")]
    parked: OnceLock<super::a2a::Task>,
}

impl CallSink {
    /// The term the dispatch minted, once it has.
    pub(crate) fn term(&self) -> Option<&str> {
        self.term.get().map(String::as_str)
    }

    /// Has a door's cancel begun for this call? (Its forward may already have stopped the call.)
    #[cfg_attr(not(feature = "a2a"), allow(dead_code))] // read by the `/a2a` door's `TaskReservation::complete`
    pub(crate) fn cancel_asked(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Park the send's answer while the cancel decides; `false` if one is parked already.
    #[cfg(feature = "a2a")]
    pub(crate) fn park(&self, task: super::a2a::Task) -> bool {
        self.parked.set(task).is_ok()
    }

    /// The send's answer, if one was parked while the cancel ran.
    #[cfg(feature = "a2a")]
    pub(crate) fn parked(&self) -> Option<&super::a2a::Task> {
        self.parked.get()
    }

    /// **What the provider answered the door's forward**, waiting up to `within_ms` for it (the forward times itself
    /// out at [`CANCEL_FORWARD_TIMEOUT_MS`]). `None`: no answer was noted in time — read as no stop seen.
    #[cfg_attr(not(feature = "a2a"), allow(dead_code))] // read by the `/a2a` door
    pub(crate) async fn forwarded_class(&self, within_ms: u64) -> Option<CancelClass> {
        let deadline = mycelium_core::sim_seam::mono_instant() + Duration::from_millis(within_ms);
        loop {
            let noted = self.forwarded_note.notified();
            tokio::pin!(noted);
            noted.as_mut().enable();
            if let Some(class) = self.forwarded.get() {
                return Some(class.clone());
            }
            let left = deadline.saturating_duration_since(mycelium_core::sim_seam::mono_instant());
            if left.is_zero() {
                return None;
            }
            tokio::select! {
                _ = noted => {}
                _ = mycelium_core::sim_seam::sleep_ms("authority/cancel-forwarded", left.as_millis() as u64) => {}
            }
        }
    }

    fn note_forwarded(&self, class: &CancelClass) {
        let _ = self.forwarded.set(class.clone());
        self.forwarded_note.notify_waiters();
    }
}

tokio::task_local! {
    /// The handle of the call the current door is dispatching, if the door holds one (`/mcp`, `/a2a`): set around the
    /// dispatch with `CALL_SINK.scope(..)`, read by `gateway_rpc_call_with_mandate` once the mint is on disk.
    pub(crate) static CALL_SINK: Arc<CallSink>;
}

/// The dispatch's half: the call of `term` is minted and its records are on disk. Fills the current door's sink, and
/// answers whether the door has cancelled the call already — then the term is revoked here, and the call must not be
/// sent. `false` with no sink in scope.
pub(crate) async fn cancelled_before_sent(ctx: &TaskCtx, term: &str) -> bool {
    let Ok(sink) = CALL_SINK.try_with(Arc::clone) else { return false };
    let _ = sink.term.set(term.to_string());
    std::sync::atomic::fence(Ordering::SeqCst);
    if !sink.cancelled.load(Ordering::SeqCst) {
        return false;
    }
    // Nothing was sent, so there is nothing to forward: the revocation on disk is the whole cancel.
    if let Err(e) = super::call_issuer::revoke(ctx, term).await {
        tracing::warn!(term, "a call cancelled before it was sent could not be revoked on disk: {e:?}");
    }
    true
}

/// What a door's cancel did.
#[derive(Debug)]
pub(crate) enum DoorCancel {
    /// The term was revoked on disk; the forward, sent before the write, runs on, its receipt counted
    /// (`call_cancel_receipts`).
    Revoked,
    /// The call is not minted yet: the dispatch, which checks the flag once the mint is on disk, revokes it and sends
    /// nothing. Or this gateway does not mint for the call, and there is nothing to revoke.
    BeforeMint,
}

/// **The door's half of a cancel**: set the flag, and if the call is minted revoke its term — sign the revocation,
/// **forward it on a task of its own at once** (§9 O10), and record it on disk, awaited, so the door answers only once
/// that is recorded. A door answers what is known when it answers (Q2): that the gateway revoked and recorded the
/// cancel, never the provider's stop. `Err(NotDurable)`: the disk refused, and the forward was sent all the same.
pub(crate) async fn cancel_at_door(ctx: &Arc<TaskCtx>, sink: &Arc<CallSink>) -> Result<DoorCancel, CancelRefusal> {
    sink.cancelled.store(true, Ordering::SeqCst);
    std::sync::atomic::fence(Ordering::SeqCst);
    let Some(term) = sink.term().map(str::to_string) else { return Ok(DoorCancel::BeforeMint) };
    let pending = super::call_issuer::begin_revoke(ctx, &term)?;
    let (ctx2, checkpoint, target, sink2, window_end_ms, term2) = (Arc::clone(ctx), pending.checkpoint.clone(), pending.target.clone(), Arc::clone(sink), pending.window_end_ms, term.clone());
    ctx.spawn_task(async move {
        let settle = |class: CancelClass| {
            tracing::debug!(term = %term, class = class.label(), "a door's cancel was forwarded");
            note_receipt(&ctx2, &class);
            sink2.note_forwarded(&class);
        };
        match target {
            Some(target) => forward_once(&ctx2, &term2, target, &checkpoint, window_end_ms, settle).await,
            None => settle(CancelClass::NotForwarded),
        }
    });
    super::call_issuer::finish_revoke(ctx, &pending).await?;
    Ok(DoorCancel::Revoked)
}

// ── the `/mcp` door's table ─────────────────────────────────────────────────────────────────────────────────────

/// The `/mcp` door's key for one call: the caller's principal, the `Mcp-Session-Id` it sent (empty without one) and
/// its JSON-RPC request id as JSON text — so another principal's `notifications/cancelled` for the same id finds
/// nothing.
pub(crate) type McpCallKey = (String, String, String);

/// One entry: the call's handle and when it was registered (`hlc.decision_now_ms()`).
pub(crate) struct McpCall {
    sink: Arc<CallSink>,
    at_ms: u64,
}

/// The table and its counters, on `CallIssuer`.
#[derive(Default)]
pub(crate) struct McpCancels {
    calls: papaya::HashMap<McpCallKey, McpCall>,
    /// A `notifications/cancelled` that found no call of its key — another principal's, a finished call, an id never
    /// sent: nothing is cancelled.
    pub(crate) unmatched: AtomicU64,
    /// A call not registered because the table held [`MAX_MCP_CANCEL_ENTRIES`] after evicting the stale: it runs, and
    /// stops only at its window.
    pub(crate) full: AtomicU64,
    /// A call not registered because a call of the same key was in flight: it runs, and stops only at its window.
    pub(crate) collisions: AtomicU64,
    /// Cancels this door revoked.
    pub(crate) cancelled: AtomicU64,
    /// Entries in `calls`, reserved before each insert: the cap's atomic half.
    count: std::sync::atomic::AtomicUsize,
    /// When the last eviction scan ran (`hlc.decision_now_ms()`): at most one a second.
    last_evict_ms: AtomicU64,
}

/// Removes its entry from the table when the call's dispatch ends — or its request is dropped.
pub(crate) struct McpRegistration {
    ctx: Arc<TaskCtx>,
    key: McpCallKey,
    sink: Arc<CallSink>,
}

impl McpRegistration {
    /// The call's handle, to scope its dispatch with.
    pub(crate) fn sink(&self) -> Arc<CallSink> {
        Arc::clone(&self.sink)
    }
}

impl Drop for McpRegistration {
    fn drop(&mut self) {
        let sink = Arc::clone(&self.sink);
        // Only this registration's entry: a key re-registered after a collision is not ours.
        let t = &self.ctx.call_issuer.mcp_cancels;
        if matches!(t.calls.pin().remove_if(&self.key, |_, v| Arc::ptr_eq(&v.sink, &sink)), Ok(Some(_))) {
            t.count.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// **Register a `/mcp` call for cancellation** before its preflight, so a cancel during the preflight is caught.
/// `None` — counted — when the key is in flight already or the table is full after evicting entries older than
/// `call_mandate_ttl_secs`: that call runs and stops only at its window.
pub(crate) fn register_mcp_call(ctx: &Arc<TaskCtx>, key: McpCallKey) -> Option<McpRegistration> {
    let t = &ctx.call_issuer.mcp_cancels;
    let now = ctx.hlc.decision_now_ms();
    // The cap is a reservation (#799's review, L5): a place is taken before the insert, so concurrent registrations
    // cannot pass it, and given back if nothing is inserted.
    if !reserve(&t.count) {
        // Full: evict what is past the window — at most one scan a second, so a full table under load does not scan
        // its 65 536 entries per registration — and try once more.
        let last = t.last_evict_ms.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= 1_000 && t.last_evict_ms.compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            let ttl_ms = ctx.config.call_mandate_ttl_secs.saturating_mul(1_000);
            // Each with its handle: a key re-registered between the scan and the removal holds a fresh call, which
            // must keep its cancel handle (#799's confirm round, L5) — so only the entry the scan saw is removed.
            let stale: Vec<(McpCallKey, Arc<CallSink>)> = t
                .calls
                .pin()
                .iter()
                .filter(|(_, v)| now.saturating_sub(v.at_ms) > ttl_ms)
                .map(|(k, v)| (k.clone(), Arc::clone(&v.sink)))
                .collect();
            #[cfg(test)]
            test_hooks::after_evict_scan(&ctx.node_id);
            for (k, seen) in stale {
                if matches!(t.calls.pin().remove_if(&k, |_, v| Arc::ptr_eq(&v.sink, &seen)), Ok(Some(_))) {
                    t.count.fetch_sub(1, Ordering::AcqRel);
                }
            }
        }
        if !reserve(&t.count) {
            t.full.fetch_add(1, Ordering::Relaxed);
            #[cfg(feature = "metrics")]
            metrics::counter!("mycelium_gateway_mcp_cancel_table_full_total").increment(1);
            return None;
        }
    }
    let sink = Arc::new(CallSink::default());
    match t.calls.pin().try_insert(key.clone(), McpCall { sink: Arc::clone(&sink), at_ms: now }) {
        Ok(_) => Some(McpRegistration { ctx: Arc::clone(ctx), key, sink }),
        Err(_) => {
            t.count.fetch_sub(1, Ordering::AcqRel);
            t.collisions.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

/// Take one place in the table, if one is free.
fn reserve(count: &std::sync::atomic::AtomicUsize) -> bool {
    count.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < MAX_MCP_CANCEL_ENTRIES).then_some(n + 1)).is_ok()
}

/// **`notifications/cancelled` at `/mcp`**: the caller's own call of that key is cancelled — its term revoked on disk
/// and forwarded. A key that names no call in flight (another principal's, a finished one) is counted and cancels
/// nothing. `Ok(None)`: unmatched.
pub(crate) async fn cancel_mcp_call(ctx: &Arc<TaskCtx>, key: &McpCallKey) -> Result<Option<DoorCancel>, CancelRefusal> {
    let t = &ctx.call_issuer.mcp_cancels;
    let Some(sink) = t.calls.pin().get(key).map(|c| Arc::clone(&c.sink)) else {
        t.unmatched.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_gateway_mcp_cancels_unmatched_total").increment(1);
        return Ok(None);
    };
    let done = cancel_at_door(ctx, &sink).await?;
    t.cancelled.fetch_add(1, Ordering::Relaxed);
    Ok(Some(done))
}

// ── the provider's half ─────────────────────────────────────────────────────────────────────────────────────────

/// A provider's refusal of a cancel-forward, as its answer.
fn refused(ctx: &TaskCtx, reason: &'static str, detail: &str) -> Vec<u8> {
    ctx.call_issuer.cancel_forwards_refused.pin().update_or_insert(reason, |n| n + 1, 1);
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_provider_call_cancel_forwards_refused_total", "reason" => reason).increment(1);
    json!({ "v": 1, "class": "refused", "reason": reason, "detail": detail }).to_string().into_bytes()
}

/// **Who sent this cancel-forward, if it is the term's issuing gateway** — the request in an envelope its sender
/// signed as itself (`principal` = `node:{via}`, `via` its verified sender), and the checkpoint that gateway's. `Err`:
/// the refusal to answer with.
/// What `admit` hands on: the issuer, its checkpoint, and whether the reply may ride the control lane — the request
/// passed its receive boundary signed as its own sender (`via` is the frame's `sender`).
type Admitted = (PrincipalId, SignedRevocationCheckpoint, bool);

fn admit(ctx: &TaskCtx, req: &super::rpc::RpcRequest) -> Result<Admitted, Vec<u8>> {
    let Ok(v) = serde_json::from_slice::<Value>(&req.payload()) else {
        return Err(refused(ctx, "malformed", "the cancel-forward is not JSON"));
    };
    let Some(signed) = v.get("checkpoint").cloned().and_then(|c| serde_json::from_value::<SignedRevocationCheckpoint>(c).ok()) else {
        return Err(refused(ctx, "malformed", "the cancel-forward carries no revocation checkpoint"));
    };
    let (sender, via_is_sender) = match super::gateway_caller::verify(ctx, req) {
        Ok(Some(c)) if matches!(c.attestation, super::gateway_caller::CallerAttestation::Signed { .. }) && c.principal == node_principal(&c.via) => {
            (node_principal(&c.via), &c.via == req.sender())
        }
        _ => return Err(refused(ctx, "cancel_not_from_issuer", "a cancel-forward is accepted only in an envelope its sender signed as itself")),
    };
    if signed.checkpoint.authority.as_str() != sender {
        return Err(refused(ctx, "cancel_not_from_issuer", "the cancel-forward's sender is not the gateway that issued the term"));
    }
    let Some(by) = PrincipalId::new(&sender) else { return Err(refused(ctx, "cancel_not_from_issuer", "the sender names no principal")) };
    if ctx.execution_authority.get().is_none() {
        return Err(refused(ctx, "no_authority", "this node has no execution authority, so it holds no call to cancel"));
    }
    Ok((by, signed, via_is_sender))
}

/// Hold the revocation, ask the work to stop, and say what happened — waiting up to [`CANCEL_CONFIRM_WAIT_MS`] for
/// work whose agent promised to stop to end, if `wait`.
async fn answer(ctx: &TaskCtx, by: &PrincipalId, signed: &SignedRevocationCheckpoint, wait: bool) -> Vec<u8> {
    #[cfg(test)]
    if test_hooks::take_panic(&test_hooks::PANIC_IN_ANSWER, &ctx.node_id) {
        panic!("a test's planted panic in the provider's answer");
    }
    let Some(authority) = ctx.execution_authority.get() else { return refused(ctx, "no_authority", "no execution authority") };
    // A provider that does not enforce registers no work and checks no admission (#799's review, M1): it cannot see
    // whether the call runs here, and a hold would refuse nothing. It says so, and holds nothing.
    if !ctx.provider_enforcement.load(Ordering::Acquire) {
        return json!({ "v": 1, "class": "not_registered" }).to_string().into_bytes();
    }
    let now = ctx.hlc.decision_now_ms();
    let class = match authority.hold_call_revocation(by, signed, now, &super::gateway_member_keys(ctx)) {
        CallRevocationHeld::Malformed => return refused(ctx, "malformed", "not a revoking checkpoint for one call: scope"),
        CallRevocationHeld::NotVerified => return refused(ctx, "not_verified", "the checkpoint does not verify under the issuing gateway's node key"),
        CallRevocationHeld::NotHeld => "not_held",
        CallRevocationHeld::Held { registered } => {
            authority.sweep(now);
            let scope = &signed.checkpoint.scope;
            match authority.call_registrations(by, scope) {
                // Nothing registered runs under the term now. If it ran here and was stopped — by this sweep or, a
                // moment earlier, by the revocation the KV carried — its stop record says how. Otherwise it ran here
                // registered and ended (`not_running`), or this node never registered it (`not_registered`): it has not
                // arrived, or it runs here unregistered — a plain rule's call whose `call:` slot did not establish —
                // and this node cannot say which (#799's review, M1).
                (0, 0) => match signed.checkpoint.scope.strip_prefix(crate::mandate::grant::CALL_SCOPE_PREFIX).and_then(|t| authority.call_stop_recorded(by, scope, t)) {
                    Some(true) => "stopped",
                    Some(false) => "unbounded",
                    None if registered => "not_running",
                    None => "not_registered",
                },
                (_, u) if u > 0 => "unbounded",
                _ if !wait => "stop_requested",
                _ => {
                    let deadline = mycelium_core::sim_seam::mono_instant() + Duration::from_millis(CANCEL_CONFIRM_WAIT_MS);
                    loop {
                        let ended = authority.work_ended();
                        // A registration whose agent promised to stop can still end as a release the provider cannot
                        // confirm — an SDK subscriber that answered rather than confirmed its stop (PR 3b's
                        // `served_call`) — so the end is read from its stop record, not from the count alone.
                        match authority.call_registrations(by, scope) {
                            (_, u) if u > 0 => break "unbounded",
                            (0, _) => {
                                let term = scope.strip_prefix(crate::mandate::grant::CALL_SCOPE_PREFIX).unwrap_or_default();
                                break if authority.call_stop_recorded(by, scope, term) == Some(false) { "unbounded" } else { "stopped" };
                            }
                            _ => {}
                        }
                        let left = deadline.saturating_duration_since(mycelium_core::sim_seam::mono_instant());
                        if left.is_zero() {
                            break "stop_requested";
                        }
                        tokio::select! {
                            _ = ended => {}
                            _ = mycelium_core::sim_seam::sleep_ms("authority/cancel-confirm", left.as_millis() as u64) => {}
                        }
                    }
                }
            }
        }
    };
    json!({ "v": 1, "class": class }).to_string().into_bytes()
}

/// **Serve the cancel-forward** on every node, for the life of the agent: a gateway cancelling a call it sent here.
/// The receive boundary (`rpc_rx`) verifies the caller envelope first; each verified request is answered on a task of
/// its own, so one cancel's wait does not hold the next.
pub(crate) fn serve(agent: &crate::GossipAgent) {
    let mut rx = agent.service().rpc_rx(CANCEL_RPC_KIND);
    let ctx = Arc::clone(&agent.service().ctx);
    let waits = Arc::new(tokio::sync::Semaphore::new(MAX_CONFIRM_WAITS));
    // §9 O13 (#826's review, F1): one answer in progress per `(issuer, scope)`. A gateway re-sends a forward it has not
    // heard back from, so a repeat can arrive while the first is still waiting for the work to end; it joins that wait
    // and takes no permit of its own. Otherwise a re-send finding no permit was answered `stop_requested` at once, and
    // that answer settled the gateway's receipt ahead of the first attempt's waited `stopped`.
    let answering: Arc<papaya::HashMap<(String, String), Arc<Answering>>> = Arc::new(papaya::HashMap::new());
    // Detached, as `replica_sync::serve` is: the loop ends when `rpc_rx` yields `None` at shutdown.
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            #[cfg(test)]
            test_hooks::note_served(&ctx.node_id);
            match admit(&ctx, &req) {
                // A refusal goes by the shared path only (#832's round-3 review): its request's `sender` is unproven.
                Err(reply) => super::rpc::rpc_respond_ctx(&ctx, &req, reply),
                Ok((by, signed, lane)) => {
                    // B1: the answer rides the control lane back only for a request signed as its own sender.
                    let respond = move |ctx: &TaskCtx, req: &super::rpc::RpcRequest, reply: Vec<u8>| {
                        if lane { super::rpc::rpc_respond_control(ctx, req, reply) } else { super::rpc::rpc_respond_ctx(ctx, req, reply) }
                    };
                    let ctx = Arc::clone(&ctx);
                    let key = (by.as_str().to_string(), signed.checkpoint.scope.clone());
                    let mine = Arc::new(Answering::default());
                    let joined = match answering.pin().try_insert(key.clone(), Arc::clone(&mine)) {
                        Ok(_) => None,
                        Err(occupied) => Some(Arc::clone(occupied.current)),
                    };
                    match joined {
                        Some(lead) => {
                            ctx.call_issuer.cancel_forwards.provider_joined.fetch_add(1, Ordering::Relaxed);
                            tokio::spawn(async move {
                                // Bounded: the answer it joined waits at most `CANCEL_CONFIRM_WAIT_MS`. Past that (its
                                // task gone), this request is answered on its own, without a wait.
                                let reply = match lead.wait(CANCEL_CONFIRM_WAIT_MS + 1_000).await {
                                    Some(reply) => reply,
                                    None => answer(&ctx, &by, &signed, false).await,
                                };
                                respond(&ctx, &req, reply);
                            });
                        }
                        None => {
                            let permit = Arc::clone(&waits).try_acquire_owned().ok();
                            #[cfg(test)]
                            let permit = permit.filter(|_| !test_hooks::over_confirm_cap(&ctx.node_id, MAX_CONFIRM_WAITS - waits.available_permits()));
                            let answering = Arc::clone(&answering);
                            tokio::spawn(async move {
                                // Removed however this task ends — a panic in `answer` included (#826's confirm round):
                                // an entry left behind would hold every later repeat of the term to its 2 s wait.
                                let _entry = Entry { map: &answering, key: key.clone(), mine: Arc::clone(&mine), on_drop: Answering::abandon };
                                let reply = answer(&ctx, &by, &signed, permit.is_some()).await;
                                drop(permit);
                                mine.done(&reply);
                                respond(&ctx, &req, reply);
                            });
                        }
                    }
                }
            }
        }
    });
}

/// **An in-progress entry's guard** (#826's confirm round): removes `mine` from `map` when dropped — at the end of its
/// task, or as a panic unwinds it — and only if it is still `mine` there; `on_drop` first tells anyone who joined it.
struct Entry<'a, K: std::hash::Hash + Eq, V> {
    map: &'a papaya::HashMap<K, Arc<V>>,
    key: K,
    mine: Arc<V>,
    on_drop: fn(&V),
}

impl<K: std::hash::Hash + Eq, V> Drop for Entry<'_, K, V> {
    fn drop(&mut self) {
        (self.on_drop)(&self.mine);
        let _ = self.map.pin().remove_if(&self.key, |_, v| Arc::ptr_eq(v, &self.mine));
    }
}

/// One provider answer in progress, which repeats of the same forward join (§9 O13).
#[derive(Default)]
struct Answering {
    reply: OnceLock<Vec<u8>>,
    note: tokio::sync::Notify,
    /// Its task ended without an answer (a panic): a joiner answers on its own at once.
    abandoned: AtomicBool,
}

impl Answering {
    fn done(&self, reply: &[u8]) {
        let _ = self.reply.set(reply.to_vec());
        self.note.notify_waiters();
    }

    fn abandon(&self) {
        if self.reply.get().is_none() {
            self.abandoned.store(true, Ordering::SeqCst);
            self.note.notify_waiters();
        }
    }

    /// The answer, once given, waiting up to `within_ms` for it.
    async fn wait(&self, within_ms: u64) -> Option<Vec<u8>> {
        let deadline = mycelium_core::sim_seam::mono_instant() + Duration::from_millis(within_ms);
        loop {
            let noted = self.note.notified();
            tokio::pin!(noted);
            noted.as_mut().enable();
            if let Some(reply) = self.reply.get() {
                return Some(reply.clone());
            }
            if self.abandoned.load(Ordering::SeqCst) {
                return None;
            }
            let left = deadline.saturating_duration_since(mycelium_core::sim_seam::mono_instant());
            if left.is_zero() {
                return None;
            }
            tokio::select! {
                _ = noted => {}
                _ = mycelium_core::sim_seam::sleep_ms("authority/cancel-answer-joined", left.as_millis() as u64) => {}
            }
        }
    }
}

impl crate::GossipAgent {
    /// **Cancel a call this gateway dispatched** (identity-and-lifecycle I4): its `call:` term, `c{n}`, is revoked,
    /// the revocation forwarded to the provider the call went to **at once** and recorded on disk beside it (§9 O10);
    /// the provider's answer is the receipt's [`CancelClass`], returned once the revocation is on disk. The receipt
    /// names what each agent answered for itself and nothing above it — see [`CancelClass`].
    /// `Err(CancelRefusal::NotDurable)`: the revocation was forwarded — the call may have been stopped — and is **not
    /// on disk**, so it lasts only as long as the processes that hold it, at most to the call's window; call again.
    /// `Err` otherwise: nothing was revoked or forwarded. A gateway that crashes before the write returns answers
    /// nothing; its revocation is then in no KV after its restart (the module doc's table).
    ///
    /// Only a gateway with `gateway_mint_call_mandates` on holds terms to cancel. Called again for the same term, the
    /// revoke is idempotent and the forward is sent again (joining the term's forward while it still waits for its
    /// receipt, §9 O13). Counted by class in `GET /stats` `call_cancel_receipts`.
    pub async fn cancel_call(&self, term: &str) -> Result<CancelReceipt, CancelRefusal> {
        cancel(&self.task_ctx, term).await
    }
}

/// Test hooks.
#[cfg(test)]
pub(crate) mod test_hooks {
    /// Per provider node: how many cancel-forward requests its `serve` loop has taken from `rpc_rx` (B1's T5: a
    /// request sent on the lane and by the shared path is served once).
    pub(crate) static SERVED: std::sync::LazyLock<papaya::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicU64>>> =
        std::sync::LazyLock::new(papaya::HashMap::new);

    pub(super) fn note_served(node: &crate::node_id::NodeId) {
        let c = std::sync::Arc::clone(SERVED.pin().get_or_insert_with(node.to_string(), Default::default));
        c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// How many cancel-forwards `node` has served.
    pub(crate) fn served(node: &crate::node_id::NodeId) -> u64 {
        SERVED.pin().get(&node.to_string()).map_or(0, |c| c.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// Per gateway node: forwards are not sent (the receipt answers `DeliveryUnknown`), so a test sees what the KV
    /// path alone does.
    pub(crate) static SUPPRESS_FORWARD: std::sync::LazyLock<papaya::HashMap<String, ()>> = std::sync::LazyLock::new(papaya::HashMap::new);

    pub(super) fn forward_suppressed(node: &crate::node_id::NodeId) -> bool {
        SUPPRESS_FORWARD.pin().contains_key(&node.to_string())
    }

    /// Per gateway node: each forward waits at its start for a permit (a test's way of delaying it until the call has
    /// completed on its own). Closed, it lets every forward by.
    pub(crate) static HOLD_FORWARD: std::sync::LazyLock<papaya::HashMap<String, std::sync::Arc<tokio::sync::Semaphore>>> = std::sync::LazyLock::new(papaya::HashMap::new);

    pub(super) async fn forward_gate(node: &crate::node_id::NodeId) {
        let gate = HOLD_FORWARD.pin().get(&node.to_string()).cloned();
        if let Some(gate) = gate
            && let Ok(permit) = gate.acquire().await
        {
            permit.forget();
        }
    }

    /// Per gateway node: forwards are sent, and their answers are lost (the receipt answers `DeliveryUnknown`).
    pub(crate) static LOSE_FORWARD_ANSWER: std::sync::LazyLock<papaya::HashMap<String, ()>> = std::sync::LazyLock::new(papaya::HashMap::new);

    /// Per gateway node (§9 O13): the answers of this many attempts are lost, the first ones — the provider acted on
    /// each, and the gateway heard nothing back.
    pub(crate) static LOSE_FIRST_ANSWERS: std::sync::LazyLock<papaya::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicU32>>> =
        std::sync::LazyLock::new(papaya::HashMap::new);

    pub(super) fn answer_lost(node: &crate::node_id::NodeId) -> bool {
        LOSE_FORWARD_ANSWER.pin().contains_key(&node.to_string()) || take_one(&LOSE_FIRST_ANSWERS, node)
    }

    /// Per gateway node (§9 O13): this many attempts are dropped before they leave, the first ones — a send the
    /// outbound queue dropped (#824's probe: the gateway's own queues, full behind a burst of mints).
    pub(crate) static DROP_FIRST_SENDS: std::sync::LazyLock<papaya::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicU32>>> =
        std::sync::LazyLock::new(papaya::HashMap::new);

    pub(super) fn send_dropped(node: &crate::node_id::NodeId) -> bool {
        take_one(&DROP_FIRST_SENDS, node)
    }

    fn take_one(map: &papaya::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicU32>>, node: &crate::node_id::NodeId) -> bool {
        let Some(left) = map.pin().get(&node.to_string()).cloned() else { return false };
        left.fetch_update(std::sync::atomic::Ordering::AcqRel, std::sync::atomic::Ordering::Acquire, |n| n.checked_sub(1)).is_ok()
    }

    /// Per gateway node (§9 O13): when its newest attempt was sent (`hlc.decision_now_ms()`), once the node is in the
    /// map.
    pub(crate) static LAST_ATTEMPT_AT: std::sync::LazyLock<papaya::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicU64>>> =
        std::sync::LazyLock::new(papaya::HashMap::new);

    pub(super) fn note_attempt(node: &crate::node_id::NodeId, at_ms: u64) {
        if let Some(last) = LAST_ATTEMPT_AT.pin().get(&node.to_string()).cloned() {
            last.fetch_max(at_ms, std::sync::atomic::Ordering::AcqRel);
        }
    }

    /// Per provider node (#826's review, F1): how many cancel-forward answers may wait for the work to end at once, in
    /// place of `MAX_CONFIRM_WAITS` (64), so a test reaches the cap with one waiting answer.
    pub(crate) static CONFIRM_WAITS_CAP: std::sync::LazyLock<papaya::HashMap<String, usize>> = std::sync::LazyLock::new(papaya::HashMap::new);

    pub(super) fn over_confirm_cap(node: &crate::node_id::NodeId, waiting: usize) -> bool {
        CONFIRM_WAITS_CAP.pin().get(&node.to_string()).is_some_and(|cap| waiting > *cap)
    }

    /// Per node, once each (#826's confirm round): a panic in the gateway's next forward, or the provider's next answer.
    pub(crate) static PANIC_IN_FORWARD: std::sync::LazyLock<papaya::HashMap<String, ()>> = std::sync::LazyLock::new(papaya::HashMap::new);
    pub(crate) static PANIC_IN_ANSWER: std::sync::LazyLock<papaya::HashMap<String, ()>> = std::sync::LazyLock::new(papaya::HashMap::new);

    pub(super) fn take_panic(map: &papaya::HashMap<String, ()>, node: &crate::node_id::NodeId) -> bool {
        map.pin().remove(&node.to_string()).is_some()
    }

    /// Per gateway node: run once between the `/mcp` table's eviction scan and its removals.
    #[allow(clippy::type_complexity)]
    pub(crate) static AFTER_EVICT_SCAN: std::sync::Mutex<Vec<(String, Box<dyn FnOnce() + Send>)>> = std::sync::Mutex::new(Vec::new());

    pub(super) fn after_evict_scan(node: &crate::node_id::NodeId) {
        let node = node.to_string();
        let hook = {
            let mut hooks = AFTER_EVICT_SCAN.lock().unwrap_or_else(|e| e.into_inner());
            hooks.iter().position(|(n, _)| *n == node).map(|i| hooks.remove(i).1)
        };
        if let Some(hook) = hook {
            hook();
        }
    }

    /// How many calls the `/mcp` table holds (I4, PR 4: the soak asserts it empty once every call has ended).
    #[cfg_attr(not(all(feature = "compliance", feature = "a2a")), allow(dead_code))] // read by `call_soak_tests`
    pub(crate) fn mcp_calls_held(ctx: &crate::agent::TaskCtx) -> usize {
        ctx.call_issuer.mcp_cancels.calls.len()
    }

    /// The handle the `/mcp` table holds for `key`, if any.
    #[cfg_attr(not(feature = "compliance"), allow(dead_code))] // read by `call_cancel_tests`, a `compliance` module
    pub(crate) fn mcp_sink(ctx: &crate::agent::TaskCtx, key: &super::McpCallKey) -> Option<std::sync::Arc<super::CallSink>> {
        ctx.call_issuer.mcp_cancels.calls.pin().get(key).map(|c| std::sync::Arc::clone(&c.sink))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider's answers read as their classes, a boundary refusal as `Refused`, and anything else as
    /// `DeliveryUnknown` — never a negative.
    #[test]
    fn a_providers_answer_reads_as_its_class_and_nothing_else_as_a_negative() {
        for class in ["stopped", "stop_requested", "unbounded", "not_running", "not_held"] {
            let c = read_answer(json!({ "v": 1, "class": class }).to_string().as_bytes());
            assert_eq!(c.label(), class);
        }
        assert_eq!(read_answer(br#"{"v":1,"class":"refused","reason":"cancel_not_from_issuer"}"#), CancelClass::Refused("cancel_not_from_issuer".into()));
        assert_eq!(read_answer(br#"{"error":"caller context refused: x","reason":"unsigned"}"#), CancelClass::Refused("unsigned".into()));
        for other in [&b"not json"[..], br#"{"v":1}"#, br#"{"v":1,"class":"exploded"}"#, b"[]"] {
            assert_eq!(read_answer(other), CancelClass::DeliveryUnknown, "{:?}", String::from_utf8_lossy(other));
        }
    }

    /// **§9 O13: the re-send schedule.** Four attempts leave inside *F* = 2 s and ten while the receipt waits
    /// ([`CANCEL_FORWARD_TIMEOUT_MS`]); then the gap doubles from 1 s to [`CANCEL_FORWARD_RETRY_CAP_MS`], and the
    /// [`MAX_CANCEL_FORWARD_ATTEMPTS`] reach past the default 300 s window, no further than one more cap.
    #[test]
    fn the_resend_schedule_fits_bound_1_and_ends_past_the_default_window() {
        // When attempt k (0-based) leaves, from the first.
        let at = |k: u32| (1..=k).map(retry_delay_ms).sum::<u64>();
        assert_eq!((at(1), at(3)), (500, 1_500), "four attempts inside F = 2 s");
        assert!(at(4) >= 2_000);
        assert_eq!(at(9), 4_500, "ten while the receipt waits");
        assert!(at(10) > CANCEL_FORWARD_TIMEOUT_MS);
        assert_eq!((retry_delay_ms(10), retry_delay_ms(11), retry_delay_ms(14), retry_delay_ms(15)), (1_000, 2_000, 16_000, 30_000));
        assert_eq!(retry_delay_ms(u32::MAX), CANCEL_FORWARD_RETRY_CAP_MS, "the gap is capped, never an overflow");
        let last = at(MAX_CANCEL_FORWARD_ATTEMPTS - 1);
        assert!((300_000..300_000 + CANCEL_FORWARD_RETRY_CAP_MS).contains(&last), "the last attempt at {last} ms");
    }
}
