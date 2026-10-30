//! The **external side-effect contract** (adopter-surface X5): a mutation outside the fleet, with the
//! receipt vocabulary extended one rung and the destination's **class declared before execution**.
//!
//! A destination inside this crate's reference shape commits its dedup row and its business change in
//! one transaction, so a lost answer is resolved by a retry (`Replayed`). A destination *outside* the
//! fleet — a payment provider, a cloud API, a ticket system — gives no such transaction. What it can
//! give is declared here, before the effect is sent, and recorded on the prepared record:
//!
//! | Class | Declared | On `DeliveryUnknown` | On a wrong outcome |
//! |---|---|---|---|
//! | [`Idempotent`](EffectClass::Idempotent) under the key | retry is safe within the dedup scope | probe, then at most one retry | a compensation, if one is also declared |
//! | [`Compensatable`](EffectClass::Compensatable) | a named compensating effect | probe; never retry | the compensating effect, a **new** record whose parent is the hash of the one it compensates |
//! | [`Irreversible`](EffectClass::Irreversible), unobservable | neither | the uncertainty is kept; the record stays unknown | preserve the evidence; escalate |
//!
//! The [`Reconciler`] holds the rules. It keeps a **ledger** (a node-local journal of its own — append-only,
//! fsynced, folded at open — the rung-1 checkpoint: the prepared record is on disk before the destination is
//! reached) and refuses by name,
//! counted, every move the class forbids: a retry without a probe, a second retry, a retry against a
//! destination that did not declare itself idempotent, a probe past the declared budget, a probe or
//! retry on an irreversible effect, a compensation without the evaluator's permit. **A probe is egress**
//! — it goes through the same hostname allow-list as the effect it probes — and both the apply and the
//! probe go through the replay kernel's `Input` seam, so a recording of a run that reached the
//! destination **replays without reaching it again**.
//!
//! What this is not: a reversal. A compensation is an effect of its own, authorised like any other and
//! recorded with the compensated record's hash as its parent (X4), so the closure reader shows both.
//! And it is not a real provider: the reference destinations are `mycelium-effects`' SQLite one and an HTTP
//! shape whose test harness loses responses; `docs/operations/what-is-proven.md` says so.
//!
//! **A found record binds what may be presented under its operation.** The ledger remembers the dedup
//! scope, the class and the content hash it was prepared with; a later apply, retry or compensation under
//! the same operation with other content is a [`Conflict`](EffectRefusal::Conflict), with another class or
//! scope a [`DeclarationMismatch`](ExternalRefusal::DeclarationMismatch) — the destination is reached with
//! one declaration per operation, or not at all. A record found `Prepared` — a send whose answer no one
//! has, after a crash or a concurrent applier — is treated as *unknown*, so it is probed, never re-sent.
//!
//! **The composition's check-then-act window.** `check_composition` runs before the record is prepared
//! and the destination reached; a process that pauses between them after an epoch is superseded acts late
//! by the pause — the same window every resource in this tree has (`authority-at-execution.md`), narrowed
//! and stated, not eliminated; a destination that can take the authority inside its own transaction
//! should. **The reconciler decides no policy:** an uncomposed effect's execution record carries an
//! *indeterminate* decision (no evaluator saw it); a composed one the destination's own permit (the
//! composition held); a compensation the evaluator's decision from its permit. **The calls block:** `send`,
//! `observe` and the ledger's journal appends are synchronous inside these `async fn`s, on the caller's executor —
//! a destination with a long wait belongs on a blocking thread of the caller's choosing.

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::counting::{RefusalCounts, RefusalSnapshot};
use super::{check_composition, Composition, Effect, EffectRefusal};
use crate::agent::journal::{read_journal, Journal};
use crate::mandate::ResourceAuthority;
use crate::{DedupOutcome, DestinationCommit, OperationId};
pub use crate::EgressPolicy;

/// The destination's declared class — stated before execution, recorded on the prepared record, and the
/// only thing the [`Reconciler`] consults when the destination's answer is unknown or wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectClass {
    /// A retry under the same idempotency key is safe within the destination's dedup scope. A
    /// compensation may be declared too, for a wrong outcome.
    Idempotent { compensation: Option<String> },
    /// A retry is **never** safe; a wrong or lost outcome is answered by the named compensating effect.
    Compensatable { compensation: String },
    /// Neither: the destination cannot be asked and cannot be undone. An unknown answer stays unknown.
    Irreversible,
}

impl EffectClass {
    /// The class's tag, as the ledger and the trace write it.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Idempotent { .. } => "idempotent",
            Self::Compensatable { .. } => "compensatable",
            Self::Irreversible => "irreversible",
        }
    }
    /// The compensating operation this class declares, if any.
    pub fn compensation(&self) -> Option<&str> {
        match self {
            Self::Idempotent { compensation } => compensation.as_deref(),
            Self::Compensatable { compensation } => Some(compensation),
            Self::Irreversible => None,
        }
    }
    fn parse(tag: &str, compensation: Option<String>) -> Option<Self> {
        Some(match tag {
            "idempotent" => Self::Idempotent { compensation },
            "compensatable" => Self::Compensatable { compensation: compensation? },
            "irreversible" => Self::Irreversible,
            _ => return None,
        })
    }
}

/// How long the unknown loop may run before the record is marked unresolved and escalated: a count of
/// probes and an age since the first unknown answer. Past either, no further probe or retry is attempted
/// without a new authorisation. The age is the input a later exposure policy reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeBudget {
    /// Probes answered `Unknown` before the record is unresolved.
    pub max_probes: u32,
    /// Milliseconds since the first unknown answer before the record is unresolved.
    pub max_age_ms: u64,
}

/// An effect bound for a destination outside the fleet, with its declared class and probe budget, and
/// — when it is composed — who it is attributed to and under what authority (the legs
/// `apply_composed` enforces, enforced here too).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalEffect {
    pub effect: Effect,
    pub class: EffectClass,
    pub probe_budget: ProbeBudget,
    pub composition: Option<Composition>,
}

/// What a reconciliation probe learned at the destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeAnswer {
    /// The effect happened there.
    Done,
    /// The effect did not happen there.
    NotDone,
    /// The destination could not say — the budget counts this one.
    Unknown,
}

impl ProbeAnswer {
    fn tag(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::NotDone => "not_done",
            Self::Unknown => "unknown",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "done" => Self::Done,
            "not_done" => Self::NotDone,
            _ => Self::Unknown,
        }
    }
}

/// The key a destination deduplicates on: its own scope, then the caller's operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdempotencyKey {
    pub scope: String,
    pub operation_id: OperationId,
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.scope, self.operation_id)
    }
}

/// A destination outside the fleet. `send` may answer `DeliveryUnknown`; `observe` is the reconciliation
/// probe — what the destination says about a key, asked separately from any effect.
pub trait ExternalDestination: Send + Sync {
    /// The destination's own identity, carried in every receipt and record.
    fn identity(&self) -> &str;
    /// The destination's idempotency domain: the scope every key it is sent lives in.
    fn dedup_scope(&self) -> &str;
    /// The host the effect and its probes reach, for the egress gate; `None` for a local destination.
    fn host(&self) -> Option<&str>;
    /// Send the effect under `key`. Blocking; the destination bounds its own wait and answers
    /// [`EffectRefusal::DeliveryUnknown`] when the bound passes.
    fn send(&self, key: &IdempotencyKey, effect: &Effect) -> Result<DestinationCommit, EffectRefusal>;
    /// Ask whether the effect under `key` happened. Blocking, bounded the same way.
    fn observe(&self, key: &IdempotencyKey) -> ProbeAnswer;
}

/// Where an external effect's record stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectState {
    /// Written before the destination was reached; the destination has not answered.
    Prepared,
    /// The destination committed it (or a probe found it done).
    Committed,
    /// The destination did not answer; its fate is unknown.
    Unknown,
    /// A probe found it not done; one retry is open if the class allows.
    NotDone,
    /// The destination refused or failed it; nothing is at the destination.
    Failed,
    /// The probe budget passed with the fate unknown: escalated, nothing further without a new authorisation.
    Unresolved,
    /// A compensating effect was committed for it.
    Compensated,
}

impl EffectState {
    fn tag(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Committed => "committed",
            Self::Unknown => "unknown",
            Self::NotDone => "not_done",
            Self::Failed => "failed",
            Self::Unresolved => "unresolved",
            Self::Compensated => "compensated",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "prepared" => Self::Prepared,
            "committed" => Self::Committed,
            "unknown" => Self::Unknown,
            "not_done" => Self::NotDone,
            "failed" => Self::Failed,
            "unresolved" => Self::Unresolved,
            "compensated" => Self::Compensated,
            _ => return None,
        })
    }
}

impl fmt::Display for EffectState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag())
    }
}

/// The ledger's record of one external effect — what was declared, what happened, how often it was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerRecord {
    pub operation_id: OperationId,
    pub dedup_scope: String,
    pub class: EffectClass,
    pub content_hash: u64,
    pub state: EffectState,
    pub probes: u32,
    pub retries: u32,
    /// When the first unknown answer came, if one did. The budget's age runs from here, and it is **never
    /// reset** — a record that fails, is re-applied and goes unknown again counts its age and its probes from
    /// the first episode; the conservative reading of one operation's history.
    pub first_unknown_at_ms: Option<u64>,
    pub prepared_at_ms: u64,
    /// The hash of the prepared record (`mycelium.effects/prepared/1`), fixed before execution — a
    /// compensation's parent when no evidence journal is attached.
    pub record_hash: String,
    /// The hash of the record's latest execution evidence in the journal, when one is attached.
    pub evidence_hash: Option<String>,
    /// The operation of the compensating effect committed for this one.
    pub compensated_by: Option<String>,
}

/// Why the reconciler did not do what it was asked. Each names what is true afterwards; a `_` arm must
/// **fail closed**.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExternalRefusal {
    /// The destination's own answer, including `DeliveryUnknown` (the record is now `Unknown`).
    Effect(EffectRefusal),
    /// The egress allow-list does not name the destination's host; neither the effect nor a probe left.
    EgressDenied { host: String },
    /// The record is `Unknown` and nothing has asked the destination: a retry without a probe is refused.
    RetryWithoutProbe,
    /// One retry was already made for this record.
    SecondRetry,
    /// The destination did not declare itself idempotent, so a retry is never safe.
    RetryNotIdempotent { class: &'static str },
    /// The probe budget passed: the record is now `Unresolved` and escalated.
    ProbeBudgetExhausted { probes: u32, age_ms: u64 },
    /// The record was already unresolved and escalated; nothing further without a new authorisation.
    Unresolved,
    /// An irreversible, unobservable effect: no probe and no retry; the record stays `Unknown`.
    IrreversibleUnobservable,
    /// `leave` was asked of a class that can be asked: a probe is that class's move, not a leave.
    LeaveOfObservable { class: &'static str },
    /// The record is not in the state the move needs.
    State { state: EffectState, needed: &'static str },
    /// The class declares no compensation.
    NoCompensation { class: &'static str },
    /// No evaluator permit was presented for the compensation, or the evaluator refused it.
    CompensationNotPermitted(String),
    /// The record was prepared under another declaration — scope or class — than this effect presents.
    DeclarationMismatch { what: &'static str, recorded: String, presented: String },
    /// The ledger holds no record for this operation.
    NotRecorded,
    /// The ledger itself failed; the move was not made.
    Ledger(String),
}

impl ExternalRefusal {
    /// The refusal's name, as the counter and the metric label write it.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Effect(_) => "effect",
            Self::EgressDenied { .. } => "egress_denied",
            Self::RetryWithoutProbe => "retry_without_probe",
            Self::SecondRetry => "second_retry",
            Self::RetryNotIdempotent { .. } => "retry_not_idempotent",
            Self::ProbeBudgetExhausted { .. } => "probe_budget_exhausted",
            Self::Unresolved => "unresolved",
            Self::IrreversibleUnobservable => "irreversible_unobservable",
            Self::LeaveOfObservable { .. } => "leave_of_observable",
            Self::State { .. } => "wrong_state",
            Self::NoCompensation { .. } => "no_compensation",
            Self::CompensationNotPermitted(_) => "compensation_not_permitted",
            Self::DeclarationMismatch { .. } => "declaration_mismatch",
            Self::NotRecorded => "not_recorded",
            Self::Ledger(_) => "ledger",
        }
    }
}

impl fmt::Display for ExternalRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Effect(e) => write!(f, "{e}"),
            Self::EgressDenied { host } => write!(f, "egress: {host} is not on the allow-list; the effect and its probes do not leave"),
            Self::RetryWithoutProbe => f.write_str("the effect's fate is unknown and nothing has asked the destination: probe before any retry"),
            Self::SecondRetry => f.write_str("one retry was already made for this effect; a second is refused"),
            Self::RetryNotIdempotent { class } => write!(f, "the destination declared itself {class}, not idempotent: a retry is never safe"),
            Self::ProbeBudgetExhausted { probes, age_ms } => write!(f, "the probe budget passed ({probes} probes, {age_ms} ms unknown): the record is unresolved and escalated"),
            Self::Unresolved => f.write_str("the record is unresolved and escalated; nothing further without a new authorisation"),
            Self::IrreversibleUnobservable => f.write_str("an irreversible, unobservable effect: no probe, no retry; its fate stays unknown and the evidence is kept"),
            Self::LeaveOfObservable { class } => write!(f, "the destination declared itself {class}, which can be asked: a probe is the move, not a leave"),
            Self::State { state, needed } => write!(f, "the record is {state}; this move needs {needed}"),
            Self::NoCompensation { class } => write!(f, "the class {class} declares no compensation"),
            Self::CompensationNotPermitted(why) => write!(f, "the compensation is not permitted: {why}"),
            Self::DeclarationMismatch { what, recorded, presented } => write!(f, "the record was prepared with {what} {recorded:?}; this effect presents {presented:?}"),
            Self::NotRecorded => f.write_str("the ledger holds no record for this operation"),
            Self::Ledger(e) => write!(f, "the ledger failed: {e}"),
        }
    }
}
impl std::error::Error for ExternalRefusal {}

impl From<EffectRefusal> for ExternalRefusal {
    fn from(e: EffectRefusal) -> Self {
        Self::Effect(e)
    }
}

/// The evaluator's permit for a compensation. Constructed only from a preflight that permitted
/// (`gateway` + `tls` build); in a build without one no compensation can be permitted, and the reconciler
/// refuses by name — a compensation is an effect of its own, authorised like any other.
#[derive(Clone, Debug)]
pub struct CompensationPermit {
    #[cfg(all(feature = "gateway", feature = "tls"))]
    envelope: crate::ActionEnvelope,
    #[cfg(all(feature = "gateway", feature = "tls"))]
    decision: crate::Decision,
    #[cfg(not(all(feature = "gateway", feature = "tls")))]
    _never: std::convert::Infallible,
}

#[cfg(all(feature = "gateway", feature = "tls"))]
impl CompensationPermit {
    /// Ask `evaluator` whether `actor` may run `operation` (the class's declared compensation) at the
    /// destination `destination_identity`, as the gateway would ask for any action — the envelope's
    /// mapping is the evaluator's own, its validity a minute from `now_ms`. A permit is returned only
    /// when the preflight permits; `Ok(None)` (no evaluator) and every refusal are
    /// [`ExternalRefusal::CompensationNotPermitted`].
    pub fn decide(
        evaluator: &Arc<dyn crate::ActionEvaluator>,
        actor: &str,
        via: crate::NodeId,
        destination_identity: &str,
        operation: &str,
        compensated_record_hash: &str,
        now_ms: u64,
    ) -> Result<Self, ExternalRefusal> {
        let resource = format!("destination:{destination_identity}");
        let mapping = evaluator.mapping(operation, &resource);
        let envelope = crate::ActionEnvelope::builder(actor, via, operation, &resource)
            .mapping(mapping)
            .validity(now_ms, now_ms.saturating_add(60_000))
            .causal(crate::causal::CausalFields { correlation: None, parent: Some(compensated_record_hash.to_string()) })
            .build();
        match crate::preflight(Some(evaluator), &envelope, now_ms) {
            Ok(Some(decision)) => Ok(Self { envelope, decision }),
            Ok(None) => Err(ExternalRefusal::CompensationNotPermitted("no action evaluator decided it".into())),
            Err(refusal) => Err(ExternalRefusal::CompensationNotPermitted(refusal.to_string())),
        }
    }

    /// The decision's policy revision.
    pub fn policy_revision(&self) -> &str {
        &self.decision.policy_revision
    }

    /// The envelope the evaluator decided — what the permit is *for*.
    pub fn envelope(&self) -> &crate::ActionEnvelope {
        &self.envelope
    }
}

// ── The ledger ────────────────────────────────────────────────────────────────────────────────

/// The ledger: every record's state, appended to a node-local journal of its own and folded at open.
///
/// The journal ([`crate::agent::journal`]) is append-only and fsynced, owns its file (an exclusive OS lock, so
/// a second reconciler on the path is refused by name) and truncates a torn tail before the first append — so
/// the **prepared record is on disk before the destination is reached** (the rung-1 checkpoint), and a
/// record's later states are appended after it, the fold taking the last. Within one process the map
/// serialises appliers: a second applier of an operation finds the first's `Prepared` record and treats it as
/// *unknown* — it probes and never re-sends. The map's lock is a leaf, never held across an append
/// (lock-order table row 68).
///
/// Until X9 this was a SQLite file in `mycelium-effects`; `rusqlite` stays quarantined to that crate, and the
/// contract moved into the substrate so the published binary can run it (`mycelium test --faults recovery`).
struct Ledger {
    journal: Arc<Journal>,
    records: std::sync::Mutex<HashMap<String, Stored>>,
}

/// One record as the journal stores it: the whole row, so a reader needs no history to fold it.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Stored {
    operation_id: String,
    dedup_scope: String,
    class: String,
    compensation: Option<String>,
    content_hash: u64,
    attempt_id: String,
    state: String,
    probes: u32,
    retries: u32,
    first_unknown_at_ms: Option<u64>,
    prepared_at_ms: u64,
    max_probes: u32,
    max_age_ms: u64,
    record_hash: String,
    evidence_hash: Option<String>,
    compensated_by: Option<String>,
    /// X4's correlation, carried into every record this operation produces; absent on a ledger written before it.
    #[serde(default)]
    correlation: Option<String>,
    /// The actor the effect is attributed to — its composition's verified principal (adopter-surface Y5's exposure
    /// source reads it); absent for an effect applied without a composition, and on a ledger written before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    actor: Option<String>,
}

impl Stored {
    fn record(&self) -> LedgerRecord {
        LedgerRecord {
            operation_id: OperationId::new(self.operation_id.clone()),
            dedup_scope: self.dedup_scope.clone(),
            class: EffectClass::parse(&self.class, self.compensation.clone()).unwrap_or(EffectClass::Irreversible),
            content_hash: self.content_hash,
            // An unreadable state is the safest one: nothing further happens to it.
            state: EffectState::parse(&self.state).unwrap_or(EffectState::Unresolved),
            probes: self.probes,
            retries: self.retries,
            first_unknown_at_ms: self.first_unknown_at_ms,
            prepared_at_ms: self.prepared_at_ms,
            record_hash: self.record_hash.clone(),
            evidence_hash: self.evidence_hash.clone(),
            compensated_by: self.compensated_by.clone(),
        }
    }

    fn prepared(ext: &ExternalEffect, scope: &str, now_ms: u64, record_hash: &str, correlation: Option<String>, actor: Option<String>) -> Self {
        Self {
            operation_id: ext.effect.operation_id.to_string(),
            dedup_scope: scope.to_string(),
            class: ext.class.tag().to_string(),
            compensation: ext.class.compensation().map(str::to_string),
            content_hash: ext.effect.content_hash,
            attempt_id: ext.effect.attempt_id.to_string(),
            state: EffectState::Prepared.tag().to_string(),
            probes: 0,
            retries: 0,
            first_unknown_at_ms: None,
            prepared_at_ms: now_ms,
            max_probes: ext.probe_budget.max_probes,
            max_age_ms: ext.probe_budget.max_age_ms,
            record_hash: record_hash.to_string(),
            evidence_hash: None,
            compensated_by: None,
            correlation,
            actor: actor.or_else(|| ext.composition.as_ref().map(|c| c.principal.to_string())),
        }
    }

    /// Whether this record's outcome is not known — prepared and unanswered, unknown, or escalated unresolved: what an
    /// actor's unresolved closure holds (Y5). An unreadable state reads as unresolved, the safest reading.
    fn outcome_unknown(&self) -> bool {
        matches!(EffectState::parse(&self.state).unwrap_or(EffectState::Unresolved), EffectState::Prepared | EffectState::Unknown | EffectState::Unresolved)
    }
}

impl Ledger {
    /// Open the ledger at `path`: the journal first (ownership, then the torn-tail repair), then the fold. The
    /// journal's writer is spawned on the current tokio runtime, so a reconciler is opened where it will run.
    fn open(path: &Path) -> Result<Self, ExternalRefusal> {
        let journal = Journal::open(path, "effects/ledger").map_err(|e| ExternalRefusal::Ledger(format!("open {}: {e}", path.display())))?;
        let mut records = HashMap::new();
        for (i, frame) in read_journal(path).map_err(|e| ExternalRefusal::Ledger(format!("read {}: {e}", path.display())))?.iter().enumerate() {
            // A frame this ledger cannot read is a record it would forget — and a forgotten record is an effect sent
            // again. The fold fails closed by name, as the node's WAL does (`on_unreadable = refuse`); nothing else
            // writes this journal (#677's review, finding 1).
            match serde_json::from_slice::<Stored>(frame) {
                Ok(s) => {
                    records.insert(s.operation_id.clone(), s);
                }
                Err(e) => {
                    return Err(ExternalRefusal::Ledger(format!("{}: frame {i} does not decode as a record ({e}); refusing to open — a record this ledger cannot read is an effect it would send again", path.display())));
                }
            }
        }
        Ok(Self { journal, records: std::sync::Mutex::new(records) })
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, Stored>> {
        self.records.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Every actor's unresolved closure as this ledger holds it (Y5): the attributed records whose outcome is not known,
    /// counted, and the oldest one's preparation time. Under the map's lock, read and released (row 68); nothing is
    /// appended.
    fn exposures(&self) -> HashMap<String, crate::agent::exposure::ActorExposure> {
        let mut out: HashMap<String, crate::agent::exposure::ActorExposure> = HashMap::new();
        for s in self.map().values().filter(|s| s.outcome_unknown()) {
            let Some(actor) = &s.actor else { continue };
            let e = out.entry(actor.clone()).or_default();
            *e = e.plus(crate::agent::exposure::ActorExposure { unresolved: 1, oldest_at_ms: Some(s.prepared_at_ms) });
        }
        out
    }

    /// One actor's unresolved closure (Y5) — [`exposures`](Self::exposures) for one actor, one pass under the lock.
    fn exposure_of(&self, actor: &str) -> crate::agent::exposure::ActorExposure {
        self.map()
            .values()
            .filter(|s| s.actor.as_deref() == Some(actor) && s.outcome_unknown())
            .fold(crate::agent::exposure::ActorExposure::default(), |e, s| {
                e.plus(crate::agent::exposure::ActorExposure { unresolved: 1, oldest_at_ms: Some(s.prepared_at_ms) })
            })
    }

    /// Append one record's whole row. The caller has already released the map.
    async fn write(&self, s: &Stored) -> Result<(), ExternalRefusal> {
        let bytes = serde_json::to_vec(s).map_err(ledger)?;
        self.journal.append(bytes).await.map_err(ledger)?;
        Ok(())
    }

    /// Change one record under the lock and hand back the row to append; `None` when there is no record.
    fn change(&self, op: &str, f: impl FnOnce(&mut Stored)) -> Option<Stored> {
        let mut map = self.map();
        let s = map.get_mut(op)?;
        f(s);
        Some(s.clone())
    }

    /// Write the prepared record unless one exists; return the record as it now stands and whether this
    /// call wrote it. A record found **`Prepared`** is a send whose answer no one has (a crash between the
    /// write and the send, or a concurrent applier that is sending now) and becomes `Unknown` here, so the
    /// caller probes and never re-sends; a record found **`Failed`** left nothing at the destination and is
    /// re-prepared under this effect's declaration. The map is written first and the journal second: the
    /// record is not handed back — and nothing is sent — until the append is acknowledged, and a fresh record
    /// whose append fails is taken out of the map again.
    async fn prepare(&self, ext: &ExternalEffect, scope: &str, now_ms: u64, record_hash: &str, correlation: Option<String>, actor: Option<String>) -> Result<(LedgerRecord, bool), ExternalRefusal> {
        let op = ext.effect.operation_id.as_str();
        let (row, fresh, write) = {
            let mut map = self.map();
            match map.get_mut(op) {
                Some(s) if s.state == EffectState::Prepared.tag() => {
                    s.state = EffectState::Unknown.tag().to_string();
                    s.first_unknown_at_ms.get_or_insert(now_ms);
                    (s.clone(), false, true)
                }
                Some(s) if s.state == EffectState::Failed.tag() => {
                    // The declaration is this effect's; the history — probes, retries, the first unknown, the
                    // evidence hash, the compensation link — stays, **never reset**: one operation's history is read
                    // conservatively across its episodes, as the SQLite ledger read it (#677's review, finding 2).
                    let fresh = Stored::prepared(ext, scope, now_ms, record_hash, correlation.or_else(|| s.correlation.clone()), actor);
                    s.dedup_scope = fresh.dedup_scope;
                    s.class = fresh.class;
                    s.compensation = fresh.compensation;
                    s.content_hash = fresh.content_hash;
                    s.attempt_id = fresh.attempt_id;
                    s.state = fresh.state;
                    s.prepared_at_ms = fresh.prepared_at_ms;
                    s.max_probes = fresh.max_probes;
                    s.max_age_ms = fresh.max_age_ms;
                    s.record_hash = fresh.record_hash;
                    s.correlation = fresh.correlation;
                    if fresh.actor.is_some() {
                        s.actor = fresh.actor;
                    }
                    (s.clone(), true, true)
                }
                Some(s) => (s.clone(), false, false),
                None => {
                    let s = Stored::prepared(ext, scope, now_ms, record_hash, correlation, actor);
                    map.insert(op.to_string(), s.clone());
                    (s, true, true)
                }
            }
        };
        if write && let Err(e) = self.write(&row).await {
            if fresh {
                self.map().remove(op);
            }
            return Err(e);
        }
        Ok((row.record(), fresh))
    }

    fn get(&self, op: &OperationId) -> Result<Option<LedgerRecord>, ExternalRefusal> {
        Ok(self.map().get(op.as_str()).map(Stored::record))
    }

    /// The actor `op`'s record is attributed to, if any (Y5) — a compensation inherits it.
    fn actor(&self, op: &OperationId) -> Option<String> {
        self.map().get(op.as_str()).and_then(|s| s.actor.clone())
    }

    fn correlation(&self, op: &OperationId) -> Option<String> {
        self.map().get(op.as_str()).and_then(|s| s.correlation.clone())
    }

    /// Claim the one retry a record may have, under the lock: a second claim — concurrent or later — is refused
    /// before anything is sent. The check and the increment are one step, so two retries racing on one record
    /// cannot both pass (adopter-surface X9's *two concurrent retries*).
    async fn claim_retry(&self, op: &OperationId) -> Result<(), ExternalRefusal> {
        let row = {
            let mut map = self.map();
            let s = map.get_mut(op.as_str()).ok_or(ExternalRefusal::NotRecorded)?;
            if s.retries >= 1 {
                return Err(ExternalRefusal::SecondRetry);
            }
            s.retries += 1;
            s.clone()
        };
        // A claim the journal did not take is given back, as `prepare` gives back its insert (#677's review, 12).
        if let Err(e) = self.write(&row).await {
            if let Some(s) = self.map().get_mut(op.as_str()) {
                s.retries -= 1;
            }
            return Err(e);
        }
        Ok(())
    }

    /// Claim one probe under the lock: the budget's count and age are checked and the count taken in one step, so two
    /// probes racing on a record cannot both pass a budget of one (#677's review, finding 3). `Ok` carries the probe
    /// number; `Err(ProbeBudgetExhausted)` carries what the record holds, for the caller to escalate.
    async fn claim_probe(&self, op: &OperationId, age_ms: u64) -> Result<u32, ExternalRefusal> {
        let (row, probes) = {
            let mut map = self.map();
            let s = map.get_mut(op.as_str()).ok_or(ExternalRefusal::NotRecorded)?;
            if s.probes >= s.max_probes || age_ms >= s.max_age_ms {
                return Err(ExternalRefusal::ProbeBudgetExhausted { probes: s.probes, age_ms });
            }
            s.probes += 1;
            (s.clone(), s.probes)
        };
        if let Err(e) = self.write(&row).await {
            if let Some(s) = self.map().get_mut(op.as_str()) {
                s.probes -= 1;
            }
            return Err(e);
        }
        Ok(probes)
    }

    /// Claim the compensation of `op` by `by` under the lock: the record must be committed with no compensation in
    /// flight, and the link is written in the same step, so two compensations racing under distinct ids cannot both
    /// pass (#677's review, finding 3). `Ok` carries the record as it stood.
    async fn claim_compensation(&self, op: &OperationId, by: &OperationId) -> Result<LedgerRecord, ExternalRefusal> {
        let (row, before) = {
            let mut map = self.map();
            let s = map.get_mut(op.as_str()).ok_or(ExternalRefusal::NotRecorded)?;
            let before = s.record();
            if before.state != EffectState::Committed {
                return Err(ExternalRefusal::State { state: before.state, needed: "a committed effect" });
            }
            if s.compensated_by.is_some() {
                return Err(ExternalRefusal::State { state: before.state, needed: "an effect whose compensation is not already in flight" });
            }
            s.compensated_by = Some(by.to_string());
            (s.clone(), before)
        };
        if let Err(e) = self.write(&row).await {
            if let Some(s) = self.map().get_mut(op.as_str()) {
                s.compensated_by = None;
            }
            return Err(e);
        }
        Ok(before)
    }

    /// Take a compensation's link off `op` again — a compensation that failed, or never got as far as the
    /// destination, leaves the record open to another (#677's review, finding 9).
    async fn unlink_compensation(&self, op: &OperationId, by: &OperationId) -> Result<(), ExternalRefusal> {
        let row = self.change(op.as_str(), |s| {
            if s.compensated_by.as_deref() == Some(by.as_str()) {
                s.compensated_by = None;
            }
        });
        match row { Some(r) => self.write(&r).await, None => Ok(()) }
    }

    /// Claim the leave of `op` under the lock: irreversible, unknown, and made `Unresolved` in the same step, so two
    /// leaves racing on a record write one record (#677's review, finding 3). `Ok` carries the record as it stood.
    async fn claim_leave(&self, op: &OperationId) -> Result<LedgerRecord, ExternalRefusal> {
        let (row, before) = {
            let mut map = self.map();
            let s = map.get_mut(op.as_str()).ok_or(ExternalRefusal::NotRecorded)?;
            let before = s.record();
            if before.class != EffectClass::Irreversible {
                return Err(ExternalRefusal::LeaveOfObservable { class: before.class.tag() });
            }
            match before.state {
                EffectState::Unknown => {}
                EffectState::Unresolved => return Err(ExternalRefusal::Unresolved),
                state => return Err(ExternalRefusal::State { state, needed: "an unknown fate" }),
            }
            s.state = EffectState::Unresolved.tag().to_string();
            (s.clone(), before)
        };
        if let Err(e) = self.write(&row).await {
            if let Some(s) = self.map().get_mut(op.as_str()) {
                s.state = EffectState::Unknown.tag().to_string();
            }
            return Err(e);
        }
        Ok(before)
    }

    fn budget(&self, op: &OperationId) -> Result<ProbeBudget, ExternalRefusal> {
        self.map().get(op.as_str()).map(|s| ProbeBudget { max_probes: s.max_probes, max_age_ms: s.max_age_ms }).ok_or(ExternalRefusal::NotRecorded)
    }

    async fn set_state(&self, op: &OperationId, state: EffectState, first_unknown_at_ms: Option<u64>) -> Result<(), ExternalRefusal> {
        let row = self.change(op.as_str(), |s| {
            s.state = state.tag().to_string();
            if let Some(t) = first_unknown_at_ms {
                s.first_unknown_at_ms.get_or_insert(t);
            }
        });
        match row { Some(r) => self.write(&r).await, None => Ok(()) }
    }

    #[cfg(all(feature = "gateway", feature = "tls"))]
    async fn set_evidence_hash(&self, op: &OperationId, hash: &str) -> Result<(), ExternalRefusal> {
        let row = self.change(op.as_str(), |s| s.evidence_hash = Some(hash.to_string()));
        match row { Some(r) => self.write(&r).await, None => Ok(()) }
    }

    /// A compensation committed (now, or later by probe or retry): the record it was linked to is compensated.
    async fn complete_compensation(&self, by: &OperationId) -> Result<(), ExternalRefusal> {
        let rows: Vec<Stored> = {
            let mut map = self.map();
            map.values_mut()
                .filter(|s| s.compensated_by.as_deref() == Some(by.as_str()) && s.state == EffectState::Committed.tag())
                .map(|s| { s.state = EffectState::Compensated.tag().to_string(); s.clone() })
                .collect()
        };
        for r in &rows {
            self.write(r).await?;
        }
        Ok(())
    }

    async fn set_compensated(&self, op: &OperationId, by: &OperationId) -> Result<(), ExternalRefusal> {
        let row = self.change(op.as_str(), |s| {
            s.state = EffectState::Compensated.tag().to_string();
            s.compensated_by = Some(by.to_string());
        });
        match row { Some(r) => self.write(&r).await, None => Ok(()) }
    }
}

fn ledger(e: impl fmt::Display) -> ExternalRefusal {
    ExternalRefusal::Ledger(e.to_string())
}

/// The prepared record's hash: `mycelium.effects/prepared/1` over what was declared, fixed before the
/// destination is reached, so a compensation can name its parent even with no journal attached.
pub fn prepared_record_hash(ext: &ExternalEffect, scope: &str) -> String {
    use sha2::{Digest, Sha256};
    // Each field length-prefixed, so a field holding the separator cannot collide with another declaration.
    let fields: [&str; 7] = [
        scope,
        ext.effect.operation_id.as_str(),
        ext.effect.attempt_id.as_str(),
        &format!("{:016x}", ext.effect.content_hash),
        ext.class.tag(),
        ext.class.compensation().unwrap_or("-"),
        &format!("{}/{}", ext.probe_budget.max_probes, ext.probe_budget.max_age_ms),
    ];
    let mut canonical = b"mycelium.effects/prepared/1\n".to_vec();
    for f in fields {
        canonical.extend_from_slice(&(f.len() as u64).to_be_bytes());
        canonical.extend_from_slice(f.as_bytes());
    }
    hex(&Sha256::digest(&canonical))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

// ── The seam's encoding of what the destination answered ───────────────────────────────────────

/// `DedupOutcome` is `#[non_exhaustive]` to this crate: an outcome added later is written as *fresh* —
/// the effect is at the destination either way, and *replayed* is the stronger claim.
fn dedup_tag(d: DedupOutcome) -> &'static str {
    match d {
        DedupOutcome::Replayed => "replayed",
        _ => "fresh",
    }
}

fn encode_send(r: &Result<DestinationCommit, EffectRefusal>) -> String {
    match r {
        Ok(c) => format!("ok {} {}", dedup_tag(c.dedup), c.destination),
        Err(EffectRefusal::DeliveryUnknown) => "unknown".into(),
        Err(EffectRefusal::Failed(m)) => format!("failed {m}"),
        Err(EffectRefusal::Conflict { committed_hash, presented_hash, .. }) => format!("conflict {committed_hash:x} {presented_hash:x}"),
        Err(EffectRefusal::Unauthorised { leg, reason }) => format!("unauthorised {leg} {reason}"),
        // The enum is `#[non_exhaustive]` for other crates; here every variant is above. A variant
        // added later lands as *failed* — not committed — rather than as a commit.
        #[allow(unreachable_patterns)]
        Err(other) => format!("failed {other}"),
    }
}

fn decode_send(s: &str, op: &OperationId) -> Result<DestinationCommit, EffectRefusal> {
    let mut parts = s.splitn(3, ' ');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("ok"), Some(dedup), Some(destination)) => Ok(DestinationCommit::new(
            destination,
            if dedup == "replayed" { DedupOutcome::Replayed } else { DedupOutcome::Fresh },
        )),
        (Some("unknown"), _, _) => Err(EffectRefusal::DeliveryUnknown),
        (Some("conflict"), Some(c), Some(p)) => Err(EffectRefusal::Conflict {
            operation_id: op.clone(),
            committed_hash: u64::from_str_radix(c, 16).unwrap_or(0),
            presented_hash: u64::from_str_radix(p, 16).unwrap_or(0),
        }),
        (Some("unauthorised"), Some(leg), Some(reason)) => Err(EffectRefusal::Unauthorised {
            leg: if leg == "attribution" { crate::effects::CompositionLeg::Attribution } else { crate::effects::CompositionLeg::Authority },
            reason: reason.to_string(),
        }),
        (Some("failed"), rest, more) => Err(EffectRefusal::Failed(format!("{}{}{}", rest.unwrap_or(""), if more.is_some() { " " } else { "" }, more.unwrap_or("")))),
        // An answer the decoder does not know is not a commit.
        _ => Err(EffectRefusal::Failed(format!("the seam's recorded answer is unreadable: {s:?}"))),
    }
}

// ── The reconciler ────────────────────────────────────────────────────────────────────────────

/// The side-effect contract's enforcement point: one ledger, one destination, one egress policy; every
/// move checked against the record's class and state before the destination is reached.
pub struct Reconciler<D> {
    destination: Arc<D>,
    ledger: Ledger,
    egress: EgressPolicy,
    counts: Arc<RefusalCounts>,
    #[cfg(all(feature = "gateway", feature = "tls"))]
    evidence: Option<(Arc<crate::EvidenceJournal>, crate::NodeId)>,
}

/// **The reconciler is an exposure source** (adopter-surface Y5): an actor's unresolved closure is its effects in this
/// ledger whose outcome is not known — prepared and unanswered, unknown, or unresolved — attributed by the effect's
/// composition. An effect applied without a composition names no actor and counts toward none. Attach it with
/// `GossipAgent::with_exposure_source`.
impl<D: ExternalDestination + 'static> crate::agent::exposure::ExposureSource for Reconciler<D> {
    fn exposure_of(&self, actor: &str) -> Result<crate::agent::exposure::ActorExposure, String> {
        Ok(self.ledger.exposure_of(actor))
    }

    fn exposures(&self) -> Result<Vec<(String, crate::agent::exposure::ActorExposure)>, String> {
        Ok(self.ledger.exposures().into_iter().collect())
    }
}

impl<D: ExternalDestination> Reconciler<D> {
    /// Open (or create) the ledger at `ledger_path` for `destination`, under `egress`.
    /// Open a reconciler over the ledger at `ledger_path` (a journal file of this reconciler's own; a second
    /// owner is refused by name, and a frame the ledger cannot read refuses the open), for one destination,
    /// under one egress policy. **Needs a tokio runtime**: the ledger's writer is spawned there, so a reconciler is
    /// opened where it will run — and re-opened after that runtime is gone, as a restarted node re-opens its WAL.
    pub fn open(ledger_path: impl AsRef<Path>, destination: Arc<D>, egress: EgressPolicy) -> Result<Self, ExternalRefusal> {
        Ok(Self {
            destination,
            ledger: Ledger::open(ledger_path.as_ref())?,
            egress,
            counts: Arc::new(RefusalCounts::default()),
            #[cfg(all(feature = "gateway", feature = "tls"))]
            evidence: None,
        })
    }

    /// Record every settled outcome, every probe and every compensation in `journal`, as the node `via`
    /// (enforcement point `destination`): an execution record with the receipt's rung, and the
    /// compensation's with the compensated record's hash as its parent.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    pub fn with_evidence(mut self, journal: Arc<crate::EvidenceJournal>, via: crate::NodeId) -> Self {
        self.evidence = Some((journal, via));
        self
    }

    /// The refusal counts so far.
    pub fn counts(&self) -> RefusalSnapshot {
        self.counts.snapshot()
    }

    /// The shared counter.
    pub fn shared_counts(&self) -> Arc<RefusalCounts> {
        Arc::clone(&self.counts)
    }

    /// The destination.
    pub fn destination(&self) -> &D {
        &self.destination
    }

    /// The ledger's record for `op`, if any.
    pub fn record(&self, op: &OperationId) -> Result<Option<LedgerRecord>, ExternalRefusal> {
        self.ledger.get(op)
    }

    fn key(&self, op: &OperationId) -> IdempotencyKey {
        IdempotencyKey { scope: self.destination.dedup_scope().to_string(), operation_id: op.clone() }
    }

    fn refuse(&self, r: ExternalRefusal) -> ExternalRefusal {
        self.counts.record_external(&r);
        r
    }

    fn gate_egress(&self) -> Result<(), ExternalRefusal> {
        if let Some(host) = self.destination.host()
            && !self.egress.permits_host(host)
        {
            return Err(self.refuse(ExternalRefusal::EgressDenied { host: host.to_string() }));
        }
        Ok(())
    }

    fn now_ms() -> u64 {
        crate::sim_seam::wall_now_ms()
    }

    /// The composed legs, when the effect carries a composition: refused without an authority to check
    /// against, then attribution, then authority (`check_composition`). Counted as the destination counts.
    fn check_composed(&self, ext: &ExternalEffect, authority: Option<&ResourceAuthority>, now_ms: u64) -> Result<(), ExternalRefusal> {
        let Some(composition) = &ext.composition else { return Ok(()) };
        let Some(authority) = authority else {
            let r = EffectRefusal::Unauthorised {
                leg: crate::effects::CompositionLeg::Authority,
                reason: "a composed effect needs the resource authority to check it against; none was given".into(),
            };
            self.counts.record(&r);
            return Err(ExternalRefusal::Effect(r));
        };
        if let Err(r) = check_composition(composition, authority, now_ms) {
            self.counts.record(&r);
            return Err(ExternalRefusal::Effect(r));
        }
        Ok(())
    }

    /// A found record binds what may be presented under its operation: this destination's scope, the
    /// recorded class, the recorded content.
    fn bind(&self, record: &LedgerRecord, ext: &ExternalEffect) -> Result<(), ExternalRefusal> {
        let scope = self.destination.dedup_scope();
        if record.dedup_scope != scope {
            return Err(self.refuse(ExternalRefusal::DeclarationMismatch { what: "dedup scope", recorded: record.dedup_scope.clone(), presented: scope.to_string() }));
        }
        if record.content_hash != ext.effect.content_hash {
            let r = EffectRefusal::Conflict { operation_id: ext.effect.operation_id.clone(), committed_hash: record.content_hash, presented_hash: ext.effect.content_hash };
            self.counts.record(&r);
            return Err(ExternalRefusal::Effect(r));
        }
        if record.class != ext.class {
            return Err(self.refuse(ExternalRefusal::DeclarationMismatch {
                what: "class",
                recorded: format!("{}{}", record.class.tag(), record.class.compensation().map(|c| format!(" compensated by {c}")).unwrap_or_default()),
                presented: format!("{}{}", ext.class.tag(), ext.class.compensation().map(|c| format!(" compensated by {c}")).unwrap_or_default()),
            }));
        }
        Ok(())
    }

    /// Send `ext` through the `Input` seam: made for real in a recording or a live run, **not made** in a
    /// replay, which returns the recorded answer.
    fn send_through_seam(&self, seam: &str, ext: &ExternalEffect) -> Result<DestinationCommit, EffectRefusal> {
        let key = self.key(&ext.effect.operation_id);
        let request = format!("{key} {:016x} {}", ext.effect.content_hash, ext.class.tag());
        let answer = crate::sim_seam::input(seam, &request, || encode_send(&self.destination.send(&key, &ext.effect)));
        decode_send(&answer, &ext.effect.operation_id)
    }

    /// Apply `ext` once. The record is prepared (on disk) before the destination is reached; the
    /// composition, when carried, is checked against `authority` first, and refused without one.
    ///
    /// A second apply of an operation already in the ledger does not reach the destination: the effect must
    /// match the record (scope, class, content — [`bind`](Self::bind)); a committed record answers `Replayed`
    /// from the ledger; an unknown one — a found `Prepared` record included — is refused `RetryWithoutProbe`;
    /// a failed one starts clean; the rest are refused by state.
    pub async fn apply(&self, ext: &ExternalEffect, authority: Option<&ResourceAuthority>) -> Result<DestinationCommit, ExternalRefusal> {
        self.apply_caused(ext, authority, crate::causal::CausalFields::default()).await
    }

    /// [`apply`](Self::apply) with X4's causal fields: `parent` is the hash of the record this effect acts on
    /// (a dependent effect names the one it depends on, so `mycelium explain --closure` finds it), and
    /// `correlation` is carried into every record this operation produces afterwards — the probe, the retry,
    /// the compensation, the leave. The fields are recorded; nothing here reads them (adopter-surface X9).
    pub async fn apply_caused(&self, ext: &ExternalEffect, authority: Option<&ResourceAuthority>, causal: crate::causal::CausalFields) -> Result<DestinationCommit, ExternalRefusal> {
        self.apply_attributed(ext, authority, causal, None).await
    }

    /// [`apply_caused`](Self::apply_caused) **attributed to `actor`** (adopter-surface Y5): the record names the actor
    /// the effect is done for, so the exposure policy counts it in that actor's unresolved closure while its outcome is
    /// not known. A composed effect is attributed to its composition's principal already; naming another actor for one
    /// is refused on the attribution leg, before anything is written.
    pub async fn apply_as(&self, actor: &crate::mandate::PrincipalId, ext: &ExternalEffect, authority: Option<&ResourceAuthority>, causal: crate::causal::CausalFields) -> Result<DestinationCommit, ExternalRefusal> {
        if let Some(c) = &ext.composition
            && c.principal != *actor
        {
            return Err(self.refuse(ExternalRefusal::Effect(EffectRefusal::Unauthorised {
                leg: crate::effects::CompositionLeg::Attribution,
                reason: format!("the effect is composed for {}, not {actor}", c.principal),
            })));
        }
        self.apply_attributed(ext, authority, causal, Some(actor.to_string())).await
    }

    async fn apply_attributed(&self, ext: &ExternalEffect, authority: Option<&ResourceAuthority>, causal: crate::causal::CausalFields, actor: Option<String>) -> Result<DestinationCommit, ExternalRefusal> {
        self.gate_egress()?;
        let now_ms = Self::now_ms();
        self.check_composed(ext, authority, now_ms)?;
        let scope = self.destination.dedup_scope().to_string();
        let record_hash = prepared_record_hash(ext, &scope);
        let (record, fresh) = self.ledger.prepare(ext, &scope, now_ms, &record_hash, causal.correlation, actor).await?;
        if !fresh {
            self.bind(&record, ext)?;
            match record.state {
                EffectState::Committed => return Ok(DestinationCommit::new(self.destination.identity(), DedupOutcome::Replayed)),
                EffectState::Unknown | EffectState::Prepared => return Err(self.refuse(ExternalRefusal::RetryWithoutProbe)),
                EffectState::Unresolved => return Err(self.refuse(ExternalRefusal::Unresolved)),
                state => return Err(self.refuse(ExternalRefusal::State { state, needed: "a record not yet sent, or one that failed" })),
            }
        }
        self.send_and_settle("effects/apply", ext, causal.parent, None).await
    }

    async fn send_and_settle(&self, seam: &str, ext: &ExternalEffect, parent: Option<String>, permit: Option<&CompensationPermit>) -> Result<DestinationCommit, ExternalRefusal> {
        let op = &ext.effect.operation_id;
        let answer = self.send_through_seam(seam, ext);
        let now_ms = Self::now_ms();
        // The record names the move, not just the door: an apply, a retry after a probe, a compensation.
        let operation = match seam {
            "effects/retry" => "effect.retry",
            "effects/compensate" => "effect.compensate",
            _ => "effect.apply",
        };
        match &answer {
            Ok(commit) => {
                self.ledger.set_state(op, EffectState::Committed, None).await?;
                // A compensation that commits — now, or a retry of one — compensates what it was linked to.
                self.ledger.complete_compensation(op).await?;
                self.counts.record_commit();
                let reason = format!("committed at {}: {}", commit.destination, dedup_tag(commit.dedup));
                self.evidence(ext, Note { operation, reason: &reason, execution: Exec::Completed, parent, now_ms, permit }).await?;
            }
            Err(EffectRefusal::DeliveryUnknown) => {
                self.ledger.set_state(op, EffectState::Unknown, Some(now_ms)).await?;
                self.counts.record(&EffectRefusal::DeliveryUnknown);
                self.evidence(ext, Note { operation, reason: "the destination did not answer; the effect's fate is unknown", execution: Exec::Attempted, parent, now_ms, permit }).await?;
            }
            Err(e) => {
                self.ledger.set_state(op, EffectState::Failed, None).await?;
                self.counts.record(e);
                self.evidence(ext, Note { operation, reason: &e.to_string(), execution: Exec::Failed, parent, now_ms, permit }).await?;
            }
        }
        answer.map_err(ExternalRefusal::Effect)
    }

    /// Ask the destination what became of `op`. Only an `Unknown` record is probed; an irreversible class
    /// is refused and stays unknown; the probe is counted before it is made, and when the budget is spent —
    /// found spent before the probe, or spent by this probe's `Unknown` answer — the record is marked
    /// `Unresolved` and escalated at once. `Done` settles the record as committed; `NotDone` opens one retry
    /// where the class allows.
    pub async fn probe(&self, op: &OperationId) -> Result<ProbeAnswer, ExternalRefusal> {
        let record = self.ledger.get(op)?.ok_or(ExternalRefusal::NotRecorded)?;
        match record.state {
            EffectState::Unknown => {}
            EffectState::Unresolved => return Err(self.refuse(ExternalRefusal::Unresolved)),
            state => return Err(self.refuse(ExternalRefusal::State { state, needed: "an unknown fate" })),
        }
        if record.class == EffectClass::Irreversible {
            return Err(self.refuse(ExternalRefusal::IrreversibleUnobservable));
        }
        let now_ms = Self::now_ms();
        let age_ms = now_ms.saturating_sub(record.first_unknown_at_ms.unwrap_or(now_ms));
        let budget = self.budget_of(op)?;
        let ext = self.ext_of(&record);
        let parent = Some(record.evidence_hash.clone().unwrap_or(record.record_hash.clone()));
        if record.probes >= budget.max_probes || age_ms >= budget.max_age_ms {
            self.escalate(op, &ext, record.probes, age_ms, parent, now_ms).await?;
            return Err(self.refuse(ExternalRefusal::ProbeBudgetExhausted { probes: record.probes, age_ms }));
        }
        self.gate_egress()?;
        // Counted before it is made: a process that dies between the probe and its answer has still spent one. The
        // claim is the budget check and the count in one step under the ledger's lock: of two probes racing on a
        // budget of one, the second is refused here and escalates (#677's review, finding 3).
        let probes = match self.ledger.claim_probe(op, age_ms).await {
            Ok(p) => p,
            Err(ExternalRefusal::ProbeBudgetExhausted { probes, age_ms }) => {
                self.escalate(op, &ext, probes, age_ms, parent, now_ms).await?;
                return Err(self.refuse(ExternalRefusal::ProbeBudgetExhausted { probes, age_ms }));
            }
            Err(e) => return Err(e),
        };
        let key = self.key(op);
        let request = format!("{key} probe {probes}");
        let answer = ProbeAnswer::parse(&crate::sim_seam::input("effects/probe", &request, || self.destination.observe(&key).tag().to_string()));
        match answer {
            ProbeAnswer::Done => {
                self.ledger.set_state(op, EffectState::Committed, None).await?;
                self.ledger.complete_compensation(op).await?;
                self.counts.record_commit();
                self.evidence(&ext, Note { operation: "effect.probe", reason: "the destination reports the effect done: settled by probe", execution: Exec::Completed, parent, now_ms, permit: None }).await?;
            }
            ProbeAnswer::NotDone => {
                self.ledger.set_state(op, EffectState::NotDone, None).await?;
                // Not `None`: this point dispatched the effect; what it has is the destination's word that
                // nothing landed (AE0 §5 reads `none` only from an explicit blocked attestation).
                self.evidence(&ext, Note { operation: "effect.probe", reason: "the destination reports the effect not done", execution: Exec::Unknown, parent, now_ms, permit: None }).await?;
            }
            ProbeAnswer::Unknown => {
                self.evidence(&ext, Note { operation: "effect.probe", reason: "the destination could not say", execution: Exec::Unknown, parent: parent.clone(), now_ms, permit: None }).await?;
                if probes >= budget.max_probes || age_ms >= budget.max_age_ms {
                    self.escalate(op, &ext, probes, age_ms, parent, now_ms).await?;
                    self.counts.record_external(&ExternalRefusal::ProbeBudgetExhausted { probes, age_ms });
                }
            }
        }
        Ok(answer)
    }

    async fn escalate(&self, op: &OperationId, ext: &ExternalEffect, probes: u32, age_ms: u64, parent: Option<String>, now_ms: u64) -> Result<(), ExternalRefusal> {
        self.ledger.set_state(op, EffectState::Unresolved, None).await?;
        self.evidence(ext, Note { operation: "effect.probe", reason: &format!("unresolved after {probes} probes and {age_ms} ms: escalated"), execution: Exec::Unresolved, parent, now_ms, permit: None }).await
    }

    /// Retry `ext` once, after a probe answered `NotDone`, where the class is idempotent. The effect must
    /// match the record ([`bind`](Self::bind)) and, when composed, hold against `authority` now. Refused by
    /// name otherwise: without a probe, a second time, or against a destination not declared idempotent.
    pub async fn retry(&self, ext: &ExternalEffect, authority: Option<&ResourceAuthority>) -> Result<DestinationCommit, ExternalRefusal> {
        let op = &ext.effect.operation_id;
        let record = self.ledger.get(op)?.ok_or(ExternalRefusal::NotRecorded)?;
        self.bind(&record, ext)?;
        if !matches!(record.class, EffectClass::Idempotent { .. }) {
            return Err(self.refuse(ExternalRefusal::RetryNotIdempotent { class: record.class.tag() }));
        }
        match record.state {
            EffectState::NotDone => {}
            EffectState::Unknown | EffectState::Prepared => return Err(self.refuse(ExternalRefusal::RetryWithoutProbe)),
            EffectState::Unresolved => return Err(self.refuse(ExternalRefusal::Unresolved)),
            state => return Err(self.refuse(ExternalRefusal::State { state, needed: "a probe that answered not done" })),
        }
        if record.retries >= 1 {
            return Err(self.refuse(ExternalRefusal::SecondRetry));
        }
        let now_ms = Self::now_ms();
        self.check_composed(ext, authority, now_ms)?;
        self.gate_egress()?;
        // The claim is the check and the increment in one step under the ledger's lock: of two retries racing on
        // this record, one is refused here before anything is sent (adopter-surface X9).
        if let Err(r) = self.ledger.claim_retry(op).await {
            return Err(self.refuse(r));
        }
        let parent = Some(record.evidence_hash.clone().unwrap_or(record.record_hash.clone()));
        self.send_and_settle("effects/retry", ext, parent, None).await
    }

    /// Commit `compensation` for the committed effect `op`, under `permit`. A compensation is a new effect
    /// in the same scope under its own operation id, recorded with the compensated record's hash as its
    /// parent; the compensated record becomes `Compensated` when it commits — now, or later by probe or
    /// retry if its answer is lost (the link is kept meanwhile, and a second compensation refused).
    ///
    /// The permit must be **for this compensation**: the class's declared operation, at this destination,
    /// naming this record as its parent, inside its validity, and — when the compensation is composed — for
    /// its principal. Refused by name when the class declares no compensation, when no permit is presented
    /// or it is for something else, or when `op` is not committed.
    /// **Leave** an irreversible effect's fate unknown — on purpose, as a record (adopter-surface X9, the
    /// third recovery verb beside the probe-and-retry and the compensation). The class's rule is that the
    /// destination can neither be asked nor undone, so the only honest move is to keep the uncertainty and
    /// escalate it: the record becomes `Unresolved`, and an evidence record (`effect.leave`,
    /// `Execution::Unresolved`) names why, with the effect's record as its parent, so the closure reader shows
    /// the decision beside the effect. Refused for a class that can be asked (`LeaveOfObservable`: a probe is
    /// that class's move), for a record whose fate is known, and for one already unresolved.
    pub async fn leave(&self, op: &OperationId) -> Result<(), ExternalRefusal> {
        // The class, the state and the change are one step under the ledger's lock (`claim_leave`).
        let record = match self.ledger.claim_leave(op).await {
            Ok(r) => r,
            Err(e) => return Err(self.refuse(e)),
        };
        let now_ms = Self::now_ms();
        let ext = self.ext_of(&record);
        let parent = Some(record.evidence_hash.clone().unwrap_or(record.record_hash.clone()));
        self.evidence(&ext, Note { operation: "effect.leave", reason: "left: irreversible and unobservable — the destination can neither be asked nor undone; the uncertainty is kept and escalated", execution: Exec::Unresolved, parent, now_ms, permit: None }).await
    }

    pub async fn compensate(
        &self,
        op: &OperationId,
        compensation: &ExternalEffect,
        permit: Option<&CompensationPermit>,
    ) -> Result<DestinationCommit, ExternalRefusal> {
        let record = self.ledger.get(op)?.ok_or(ExternalRefusal::NotRecorded)?;
        let Some(declared) = record.class.compensation() else {
            return Err(self.refuse(ExternalRefusal::NoCompensation { class: record.class.tag() }));
        };
        let Some(permit) = permit else {
            return Err(self.refuse(ExternalRefusal::CompensationNotPermitted("no evaluator permit was presented".into())));
        };
        if record.state != EffectState::Committed {
            return Err(self.refuse(ExternalRefusal::State { state: record.state, needed: "a committed effect" }));
        }
        if record.compensated_by.is_some() {
            return Err(self.refuse(ExternalRefusal::State { state: record.state, needed: "an effect whose compensation is not already in flight" }));
        }
        if compensation.effect.operation_id == *op {
            return Err(self.refuse(ExternalRefusal::State { state: record.state, needed: "a compensation under its own operation id" }));
        }
        let now_ms = Self::now_ms();
        let parent = record.evidence_hash.clone().unwrap_or(record.record_hash.clone());
        self.bind_permit(permit, declared, &parent, compensation, now_ms)?;
        // A composed compensation would need the resource authority to check against, and this door takes none: it
        // is refused by the composed check rather than sent unchecked, as `apply` and `retry` check theirs (#677's
        // review, finding 9).
        self.check_composed(compensation, None, now_ms)?;
        self.gate_egress()?;
        // The in-flight check and the link are one step under the ledger's lock: of two compensations racing under
        // distinct ids, the second is refused here (finding 3). Everything after it that fails before the destination
        // is reached — or fails there — takes the link off again, so the record stays open to another compensation.
        let by = compensation.effect.operation_id.clone();
        self.ledger.claim_compensation(op, &by).await.map_err(|e| self.refuse(e))?;
        let outcome = self.compensate_linked(op, compensation, permit, parent, now_ms).await;
        match &outcome {
            Ok(_) | Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown)) => {}
            // Nothing is at the destination (a failed answer, a conflict, a composition that did not hold, a refusal
            // before the send): the link comes off, and a later compensation may be issued.
            Err(_) => self.ledger.unlink_compensation(op, &by).await?,
        }
        outcome
    }

    /// The compensation's record, its permit's record and its send, after the link is claimed.
    async fn compensate_linked(&self, op: &OperationId, compensation: &ExternalEffect, permit: &CompensationPermit, parent: String, now_ms: u64) -> Result<DestinationCommit, ExternalRefusal> {
        self.record_permit(permit).await?;
        let scope = self.destination.dedup_scope().to_string();
        let record_hash = prepared_record_hash(compensation, &scope);
        // The compensation inherits the compensated record's correlation: one run, read as one chain.
        let (existing, fresh) = self.ledger.prepare(compensation, &scope, now_ms, &record_hash, self.ledger.correlation(op), self.ledger.actor(op)).await?;
        if !fresh {
            self.bind(&existing, compensation)?;
            return Err(self.refuse(ExternalRefusal::State { state: existing.state, needed: "a compensation not yet sent" }));
        }
        let commit = self.send_and_settle("effects/compensate", compensation, Some(parent), Some(permit)).await?;
        self.ledger.set_compensated(op, &compensation.effect.operation_id).await?;
        Ok(commit)
    }

    /// The permit is for this compensation, or it is not a permit for it.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    fn bind_permit(&self, permit: &CompensationPermit, declared: &str, parent: &str, compensation: &ExternalEffect, now_ms: u64) -> Result<(), ExternalRefusal> {
        let env = &permit.envelope;
        let resource = format!("destination:{}", self.destination.identity());
        let mismatch = |why: String| ExternalRefusal::CompensationNotPermitted(why);
        if env.operation != declared {
            return Err(self.refuse(mismatch(format!("the permit is for {:?}, the class declares {declared:?}", env.operation))));
        }
        if env.resource != resource {
            return Err(self.refuse(mismatch(format!("the permit is for {:?}, this destination is {resource:?}", env.resource))));
        }
        if env.parent.as_deref() != Some(parent) {
            return Err(self.refuse(mismatch("the permit does not name this record as its parent".into())));
        }
        if now_ms < env.issued_at_ms || env.is_expired(now_ms) {
            return Err(self.refuse(mismatch("the permit is outside its validity".into())));
        }
        if let Some(c) = &compensation.composition
            && c.principal.as_str() != env.actor
        {
            return Err(self.refuse(mismatch(format!("the permit is for {:?}, the compensation is attributed to {:?}", env.actor, c.principal.as_str()))));
        }
        Ok(())
    }
    #[cfg(not(all(feature = "gateway", feature = "tls")))]
    fn bind_permit(&self, permit: &CompensationPermit, _declared: &str, _parent: &str, _compensation: &ExternalEffect, _now_ms: u64) -> Result<(), ExternalRefusal> {
        match permit._never {}
    }

    fn budget_of(&self, op: &OperationId) -> Result<ProbeBudget, ExternalRefusal> {
        self.ledger.budget(op)
    }

    /// The effect as the ledger remembers it — enough for a record; the payload is not kept.
    fn ext_of(&self, record: &LedgerRecord) -> ExternalEffect {
        ExternalEffect {
            effect: Effect {
                operation_id: record.operation_id.clone(),
                attempt_id: crate::AttemptId::of(&record.operation_id, record.retries + 1),
                content_hash: record.content_hash,
                payload: Vec::new(),
            },
            class: record.class.clone(),
            probe_budget: ProbeBudget { max_probes: 0, max_age_ms: 0 },
            composition: None,
        }
    }

    /// The permit's decision, recorded as the evaluator made it (enforcement point `destination`).
    #[cfg(all(feature = "gateway", feature = "tls"))]
    async fn record_permit(&self, permit: &CompensationPermit) -> Result<(), ExternalRefusal> {
        let Some((journal, _)) = &self.evidence else { return Ok(()) };
        let mut ev = crate::AeEvidence::for_decision(&permit.envelope, &permit.decision, crate::Execution::Attempted, "destination");
        ev.destination = Some(self.destination.identity().to_string());
        let bytes = serde_json::to_vec(&ev).map_err(ledger)?;
        journal.append(bytes).await.map_err(|e| ExternalRefusal::Ledger(format!("the evidence journal: {e}")))?;
        Ok(())
    }
    #[cfg(not(all(feature = "gateway", feature = "tls")))]
    async fn record_permit(&self, permit: &CompensationPermit) -> Result<(), ExternalRefusal> {
        match permit._never {}
    }

    /// Write the execution record for `ext` when a journal is attached, and remember its hash on the
    /// ledger. The decision on the record is honest about who decided: a compensation carries the
    /// evaluator's (from its permit); a composed effect the destination's own permit (the composition held);
    /// an uncomposed effect an *indeterminate* decision — no policy saw it, and this record states the
    /// destination's answer, not an authorisation. The receipt's rung (`effect_durability`) is written where
    /// it is true — a local destination's commit is on this node's disk; a remote destination's commit is the
    /// destination's claim, which the local rung cannot name, and the record carries it as `destination`
    /// and the dedup outcome in `reason`. Without a journal the ledger's own record hash stands.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    async fn evidence(&self, ext: &ExternalEffect, note: Note<'_>) -> Result<(), ExternalRefusal> {
        let Some((journal, via)) = &self.evidence else { return Ok(()) };
        let Note { operation, reason, execution, parent, now_ms, permit } = note;
        let resource = format!("destination:{}", self.destination.identity());
        let (envelope, decision) = match permit {
            Some(p) => {
                let mut d = p.decision.clone();
                d.reason = reason.to_string();
                (p.envelope.clone(), d)
            }
            None => {
                let actor = ext.composition.as_ref().map(|c| c.principal.as_str().to_string()).unwrap_or_else(|| format!("destination:{}", self.destination.identity()));
                let envelope = crate::ActionEnvelope::builder(actor, via.clone(), operation, &resource)
                    .identities(ext.effect.operation_id.to_string(), ext.effect.attempt_id.to_string())
                    .validity(now_ms, now_ms.saturating_add(60_000))
                    .causal(crate::causal::CausalFields { correlation: self.ledger.correlation(&ext.effect.operation_id), parent })
                    .build();
                let decision = if ext.composition.is_some() {
                    let mut d = crate::Decision::permit(format!("the composition held at the destination; {reason}"), "-");
                    d.checked = vec!["composition:attribution".into(), "composition:authority".into()];
                    d
                } else {
                    crate::Decision::indeterminate(format!("no policy decided this effect at the destination; {reason}"), "-")
                };
                (envelope, decision)
            }
        };
        let mut ev = crate::AeEvidence::for_decision(&envelope, &decision, execution.into(), "destination").as_execution(execution.into());
        ev.destination = Some(self.destination.identity().to_string());
        ev.operation_id = ext.effect.operation_id.to_string();
        ev.attempt_id = ext.effect.attempt_id.to_string();
        if execution == Exec::Completed && self.destination.host().is_none() {
            ev = ev.with_effect_durability(crate::LocalDurability::OnDisk);
        }
        if let Some(c) = &ext.composition
            && let Some(d) = &c.origin_domain
        {
            ev = ev.with_origin_domain(d.clone());
        }
        let bytes = serde_json::to_vec(&ev).map_err(ledger)?;
        let appended = journal.append(bytes).await.map_err(|e| ExternalRefusal::Ledger(format!("the evidence journal: {e}")))?;
        self.ledger.set_evidence_hash(&ext.effect.operation_id, &hex(&appended.content_hash)).await?;
        Ok(())
    }
    #[cfg(not(all(feature = "gateway", feature = "tls")))]
    async fn evidence(&self, _ext: &ExternalEffect, _note: Note<'_>) -> Result<(), ExternalRefusal> {
        Ok(())
    }
}

/// What one evidence record says — the arguments `evidence` takes, bundled. Read only where a journal can be
/// attached; the no-`envelope` build takes the note and writes nothing.
#[cfg_attr(not(all(feature = "gateway", feature = "tls")), allow(dead_code))]
struct Note<'a> {
    operation: &'a str,
    reason: &'a str,
    execution: Exec,
    parent: Option<String>,
    now_ms: u64,
    permit: Option<&'a CompensationPermit>,
}

/// The execution states this module writes, named here so the no-`envelope` build has them too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Exec {
    Completed,
    Attempted,
    Failed,
    Unknown,
    Unresolved,
}

#[cfg(all(feature = "gateway", feature = "tls"))]
impl From<Exec> for crate::Execution {
    fn from(e: Exec) -> Self {
        match e {
            Exec::Completed => Self::Completed,
            Exec::Attempted => Self::Attempted,
            Exec::Failed => Self::Failed,
            Exec::Unknown => Self::Unknown,
            Exec::Unresolved => Self::Unresolved,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seams_encoding_round_trips_every_answer() {
        let op = OperationId::new("op-1");
        let answers: Vec<Result<DestinationCommit, EffectRefusal>> = vec![
            Ok(DestinationCommit::new("pay", DedupOutcome::Fresh)),
            Ok(DestinationCommit::new("pay x", DedupOutcome::Replayed)),
            Err(EffectRefusal::DeliveryUnknown),
            Err(EffectRefusal::Failed("boom, with spaces".into())),
            Err(EffectRefusal::Conflict { operation_id: op.clone(), committed_hash: 0xabc, presented_hash: 0xdef }),
            Err(EffectRefusal::Unauthorised { leg: crate::effects::CompositionLeg::Attribution, reason: "not the holder".into() }),
        ];
        for a in answers {
            assert_eq!(decode_send(&encode_send(&a), &op), a, "{}", encode_send(&a));
        }
        assert!(matches!(decode_send("gibberish", &op), Err(EffectRefusal::Failed(_))), "an unreadable answer is not a commit");
        for p in [ProbeAnswer::Done, ProbeAnswer::NotDone, ProbeAnswer::Unknown] {
            assert_eq!(ProbeAnswer::parse(p.tag()), p);
        }
    }

    /// A destination that is never reached: the ledger tests need a reconciler, not a destination.
    struct Nowhere;
    impl ExternalDestination for Nowhere {
        fn identity(&self) -> &str { "nowhere" }
        fn dedup_scope(&self) -> &str { "nowhere" }
        fn host(&self) -> Option<&str> { None }
        fn send(&self, _: &IdempotencyKey, _: &Effect) -> Result<DestinationCommit, EffectRefusal> { Err(EffectRefusal::Failed("never reached".into())) }
        fn observe(&self, _: &IdempotencyKey) -> ProbeAnswer { ProbeAnswer::Unknown }
    }

    fn ledger_file(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("mycelium-effects-ledger-{tag}-{}.journal", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(format!("{}.lock", p.display()));
        p
    }

    fn an_effect(op: &str, class: EffectClass) -> ExternalEffect {
        let op = OperationId::new(op);
        ExternalEffect { effect: Effect::new(op.clone(), crate::AttemptId::of(&op, 1), b"x".to_vec()), class, probe_budget: ProbeBudget { max_probes: 2, max_age_ms: 60_000 }, composition: None }
    }

    /// **A ledger frame that does not decode refuses the open** (#677's review, finding 1): a record the ledger
    /// cannot read is an effect it would send again, so the fold fails closed by name, as the node's WAL does.
    /// Seen failing first: the fold skipped the frame and the operation read as not recorded.
    #[tokio::test]
    async fn a_ledger_frame_that_does_not_decode_refuses_the_open_instead_of_forgetting_the_record() {
        let path = ledger_file("undecodable");
        {
            let j = Journal::open(&path, "effects/ledger").unwrap();
            j.append(b"{\"not\": \"a record\"}".to_vec()).await.unwrap();
        }
        match Reconciler::open(&path, Arc::new(Nowhere), EgressPolicy::default()) {
            Err(ExternalRefusal::Ledger(why)) => assert!(why.contains("does not decode") && why.contains("frame 0"), "{why}"),
            Ok(_) => panic!("a ledger with a frame it cannot read opened, and would re-send the record's effect"),
            Err(e) => panic!("refused for another reason: {e}"),
        }
    }

    /// **A failed record re-prepared keeps its history** (#677's review, finding 2): the declaration columns are
    /// the new effect's, the probes, retries, first unknown, evidence hash and compensation link stay — `never
    /// reset`, as the field's doc says and the SQLite ledger did. Seen failing first: the row was replaced whole and
    /// a second retry would have been accepted.
    #[tokio::test]
    async fn a_failed_record_re_prepared_keeps_its_probes_retries_and_first_unknown() {
        let ledger = Ledger::open(&ledger_file("failed-history")).unwrap();
        let ext = an_effect("op-7", EffectClass::Idempotent { compensation: None });
        let (_, fresh) = ledger.prepare(&ext, "nowhere", 1_000, "h1", Some("run-7".into()), None).await.unwrap();
        assert!(fresh);
        ledger.set_state(&ext.effect.operation_id, EffectState::Unknown, Some(1_000)).await.unwrap();
        ledger.claim_probe(&ext.effect.operation_id, 0).await.unwrap();
        ledger.claim_retry(&ext.effect.operation_id).await.unwrap();
        ledger.set_state(&ext.effect.operation_id, EffectState::Failed, None).await.unwrap();
        let (again, fresh) = ledger.prepare(&ext, "nowhere", 2_000, "h2", None, None).await.unwrap();
        assert!(fresh, "a failed record is re-prepared");
        assert_eq!(again.state, EffectState::Prepared);
        assert_eq!(again.prepared_at_ms, 2_000, "the declaration is this effect's");
        assert_eq!(again.record_hash, "h2");
        assert_eq!((again.probes, again.retries, again.first_unknown_at_ms), (1, 1, Some(1_000)), "the history is kept: {again:?}");
        assert_eq!(ledger.correlation(&ext.effect.operation_id).as_deref(), Some("run-7"), "the correlation is kept when the new effect carries none");
        assert!(matches!(ledger.claim_retry(&ext.effect.operation_id).await, Err(ExternalRefusal::SecondRetry)), "the one retry was spent in the first episode");
    }

    #[test]
    fn the_prepared_hash_is_fixed_by_what_was_declared() {
        let op = OperationId::new("op-2");
        let effect = Effect::new(op.clone(), crate::AttemptId::of(&op, 1), b"42".to_vec());
        let a = ExternalEffect { effect: effect.clone(), class: EffectClass::Idempotent { compensation: None }, probe_budget: ProbeBudget { max_probes: 3, max_age_ms: 1000 }, composition: None };
        let b = ExternalEffect { class: EffectClass::Compensatable { compensation: "refund".into() }, ..a.clone() };
        assert_eq!(prepared_record_hash(&a, "s"), prepared_record_hash(&a, "s"));
        assert_ne!(prepared_record_hash(&a, "s"), prepared_record_hash(&b, "s"), "the class is part of the record");
        assert_ne!(prepared_record_hash(&a, "s"), prepared_record_hash(&a, "t"), "so is the scope");
        assert_eq!(prepared_record_hash(&a, "s").len(), 64);
    }
}
