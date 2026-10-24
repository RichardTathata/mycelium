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
//! The [`Reconciler`] holds the rules. It keeps a **ledger** (a SQLite file of its own, the rung-1
//! checkpoint: the prepared record is on disk before the destination is reached) and refuses by name,
//! counted, every move the class forbids: a retry without a probe, a second retry, a retry against a
//! destination that did not declare itself idempotent, a probe past the declared budget, a probe or
//! retry on an irreversible effect, a compensation without the evaluator's permit. **A probe is egress**
//! — it goes through the same hostname allow-list as the effect it probes — and both the apply and the
//! probe go through the replay kernel's `Input` seam, so a recording of a run that reached the
//! destination **replays without reaching it again**.
//!
//! What this is not: a reversal. A compensation is an effect of its own, authorised like any other and
//! recorded with the compensated record's hash as its parent (X4), so the closure reader shows both.
//! And it is not a real provider: the reference destinations are the SQLite one and an HTTP shape whose
//! test harness loses responses; `docs/operations/what-is-proven.md` says so.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::counting::RefusalCounts;
use crate::{
    check_composition, Composition, DedupOutcome, DestinationCommit, Effect, EffectRefusal, OperationId,
    ResourceAuthority, RefusalSnapshot,
};
pub use mycelium::EgressPolicy;

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
    /// When the first unknown answer came, if one did (the budget's age runs from here).
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
    /// The record is not in the state the move needs.
    State { state: EffectState, needed: &'static str },
    /// The class declares no compensation.
    NoCompensation { class: &'static str },
    /// No evaluator permit was presented for the compensation, or the evaluator refused it.
    CompensationNotPermitted(String),
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
            Self::State { .. } => "wrong_state",
            Self::NoCompensation { .. } => "no_compensation",
            Self::CompensationNotPermitted(_) => "compensation_not_permitted",
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
            Self::State { state, needed } => write!(f, "the record is {state}; this move needs {needed}"),
            Self::NoCompensation { class } => write!(f, "the class {class} declares no compensation"),
            Self::CompensationNotPermitted(why) => write!(f, "the compensation is not permitted: {why}"),
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
/// (`envelope` feature); in a build without one no compensation can be permitted, and the reconciler
/// refuses by name — a compensation is an effect of its own, authorised like any other.
#[derive(Clone, Debug)]
pub struct CompensationPermit {
    #[cfg(feature = "envelope")]
    envelope: mycelium::ActionEnvelope,
    #[cfg(feature = "envelope")]
    decision: mycelium::Decision,
    #[cfg(not(feature = "envelope"))]
    _never: std::convert::Infallible,
}

#[cfg(feature = "envelope")]
impl CompensationPermit {
    /// Ask `evaluator` whether `actor` may run `operation` (the class's declared compensation) at the
    /// destination `destination_identity`, as the gateway would ask for any action — the envelope's
    /// mapping is the evaluator's own, its validity a minute from `now_ms`. A permit is returned only
    /// when the preflight permits; `Ok(None)` (no evaluator) and every refusal are
    /// [`ExternalRefusal::CompensationNotPermitted`].
    pub fn decide(
        evaluator: &Arc<dyn mycelium::ActionEvaluator>,
        actor: &str,
        via: mycelium::NodeId,
        destination_identity: &str,
        operation: &str,
        compensated_record_hash: &str,
        now_ms: u64,
    ) -> Result<Self, ExternalRefusal> {
        let resource = format!("destination:{destination_identity}");
        let mapping = evaluator.mapping(operation, &resource);
        let envelope = mycelium::ActionEnvelope::builder(actor, via, operation, &resource)
            .mapping(mapping)
            .validity(now_ms, now_ms.saturating_add(60_000))
            .causal(mycelium::causal::CausalFields { correlation: None, parent: Some(compensated_record_hash.to_string()) })
            .build();
        match mycelium::preflight(Some(evaluator), &envelope, now_ms) {
            Ok(Some(decision)) => Ok(Self { envelope, decision }),
            Ok(None) => Err(ExternalRefusal::CompensationNotPermitted("no action evaluator decided it".into())),
            Err(refusal) => Err(ExternalRefusal::CompensationNotPermitted(refusal.to_string())),
        }
    }

    /// The decision's policy revision.
    pub fn policy_revision(&self) -> &str {
        &self.decision.policy_revision
    }
}

// ── The ledger ────────────────────────────────────────────────────────────────────────────────

struct Ledger {
    path: PathBuf,
}

impl Ledger {
    fn open(path: &Path) -> Result<Self, ExternalRefusal> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path).map_err(ledger)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS external_effects (
                 operation_id        TEXT PRIMARY KEY NOT NULL,
                 dedup_scope         TEXT NOT NULL,
                 class               TEXT NOT NULL,
                 compensation        TEXT,
                 content_hash        INTEGER NOT NULL,
                 attempt_id          TEXT NOT NULL,
                 state               TEXT NOT NULL,
                 probes              INTEGER NOT NULL DEFAULT 0,
                 retries             INTEGER NOT NULL DEFAULT 0,
                 first_unknown_at_ms INTEGER,
                 prepared_at_ms      INTEGER NOT NULL,
                 max_probes          INTEGER NOT NULL,
                 max_age_ms          INTEGER NOT NULL,
                 record_hash         TEXT NOT NULL,
                 evidence_hash       TEXT,
                 compensated_by      TEXT
             );",
        )
        .map_err(ledger)?;
        Ok(Self { path: path.to_path_buf() })
    }

    fn connect(&self) -> Result<Connection, ExternalRefusal> {
        let conn = Connection::open(&self.path).map_err(ledger)?;
        conn.busy_timeout(Duration::from_secs(5)).map_err(ledger)?;
        Ok(conn)
    }

    /// Write the prepared record unless one exists; return the record as it now stands and whether
    /// this call wrote it.
    fn prepare(&self, ext: &ExternalEffect, scope: &str, now_ms: u64, record_hash: &str) -> Result<(LedgerRecord, bool), ExternalRefusal> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(ledger)?;
        let inserted = tx
            .execute(
                "INSERT OR IGNORE INTO external_effects
                   (operation_id, dedup_scope, class, compensation, content_hash, attempt_id, state, prepared_at_ms, max_probes, max_age_ms, record_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'prepared', ?7, ?8, ?9, ?10)",
                params![
                    ext.effect.operation_id.as_str(),
                    scope,
                    ext.class.tag(),
                    ext.class.compensation(),
                    ext.effect.content_hash as i64,
                    ext.effect.attempt_id.as_str(),
                    now_ms as i64,
                    ext.probe_budget.max_probes as i64,
                    ext.probe_budget.max_age_ms as i64,
                    record_hash,
                ],
            )
            .map_err(ledger)?;
        let record = Self::read(&tx, ext.effect.operation_id.as_str())?.ok_or(ExternalRefusal::NotRecorded)?;
        tx.commit().map_err(ledger)?;
        Ok((record, inserted == 1))
    }

    fn read(conn: &Connection, op: &str) -> Result<Option<LedgerRecord>, ExternalRefusal> {
        conn.query_row(
            "SELECT operation_id, dedup_scope, class, compensation, content_hash, state, probes, retries, first_unknown_at_ms,
                    prepared_at_ms, record_hash, evidence_hash, compensated_by
             FROM external_effects WHERE operation_id = ?1",
            params![op],
            |r| {
                let class_tag: String = r.get(2)?;
                let compensation: Option<String> = r.get(3)?;
                let state_tag: String = r.get(5)?;
                Ok(LedgerRecord {
                    operation_id: OperationId::new(r.get::<_, String>(0)?),
                    dedup_scope: r.get(1)?,
                    class: EffectClass::parse(&class_tag, compensation).unwrap_or(EffectClass::Irreversible),
                    content_hash: r.get::<_, i64>(4)? as u64,
                    // An unreadable state is the safest one: nothing further happens to it.
                    state: EffectState::parse(&state_tag).unwrap_or(EffectState::Unresolved),
                    probes: r.get::<_, i64>(6)? as u32,
                    retries: r.get::<_, i64>(7)? as u32,
                    first_unknown_at_ms: r.get::<_, Option<i64>>(8)?.map(|v| v as u64),
                    prepared_at_ms: r.get::<_, i64>(9)? as u64,
                    record_hash: r.get(10)?,
                    evidence_hash: r.get(11)?,
                    compensated_by: r.get(12)?,
                })
            },
        )
        .optional()
        .map_err(ledger)
    }

    fn get(&self, op: &OperationId) -> Result<Option<LedgerRecord>, ExternalRefusal> {
        Self::read(&self.connect()?, op.as_str())
    }

    fn set_state(&self, op: &OperationId, state: EffectState, first_unknown_at_ms: Option<u64>) -> Result<(), ExternalRefusal> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE external_effects SET state = ?2, first_unknown_at_ms = COALESCE(first_unknown_at_ms, ?3) WHERE operation_id = ?1",
            params![op.as_str(), state.tag(), first_unknown_at_ms.map(|v| v as i64)],
        )
        .map_err(ledger)?;
        Ok(())
    }

    fn bump(&self, op: &OperationId, column: &str) -> Result<(), ExternalRefusal> {
        let conn = self.connect()?;
        let sql = match column {
            "probes" => "UPDATE external_effects SET probes = probes + 1 WHERE operation_id = ?1",
            _ => "UPDATE external_effects SET retries = retries + 1 WHERE operation_id = ?1",
        };
        conn.execute(sql, params![op.as_str()]).map_err(ledger)?;
        Ok(())
    }

    #[cfg(feature = "envelope")]
    fn set_evidence_hash(&self, op: &OperationId, hash: &str) -> Result<(), ExternalRefusal> {
        let conn = self.connect()?;
        conn.execute("UPDATE external_effects SET evidence_hash = ?2 WHERE operation_id = ?1", params![op.as_str(), hash]).map_err(ledger)?;
        Ok(())
    }

    fn set_compensated(&self, op: &OperationId, by: &OperationId) -> Result<(), ExternalRefusal> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE external_effects SET state = 'compensated', compensated_by = ?2 WHERE operation_id = ?1",
            params![op.as_str(), by.as_str()],
        )
        .map_err(ledger)?;
        Ok(())
    }
}

fn ledger(e: impl fmt::Display) -> ExternalRefusal {
    ExternalRefusal::Ledger(e.to_string())
}

/// The prepared record's hash: `mycelium.effects/prepared/1` over what was declared, fixed before the
/// destination is reached, so a compensation can name its parent even with no journal attached.
pub fn prepared_record_hash(ext: &ExternalEffect, scope: &str) -> String {
    use sha2::{Digest, Sha256};
    let canonical = format!(
        "mycelium.effects/prepared/1\n{scope}\n{}\n{}\n{:016x}\n{}\n{}\n{}\n{}\n",
        ext.effect.operation_id,
        ext.effect.attempt_id,
        ext.effect.content_hash,
        ext.class.tag(),
        ext.class.compensation().unwrap_or("-"),
        ext.probe_budget.max_probes,
        ext.probe_budget.max_age_ms,
    );
    hex(&Sha256::digest(canonical.as_bytes()))
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
            leg: if leg == "attribution" { crate::CompositionLeg::Attribution } else { crate::CompositionLeg::Authority },
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
    #[cfg(feature = "envelope")]
    evidence: Option<(Arc<mycelium::EvidenceJournal>, mycelium::NodeId)>,
}

impl<D: ExternalDestination> Reconciler<D> {
    /// Open (or create) the ledger at `ledger_path` for `destination`, under `egress`.
    pub fn open(ledger_path: impl AsRef<Path>, destination: Arc<D>, egress: EgressPolicy) -> Result<Self, ExternalRefusal> {
        Ok(Self {
            destination,
            ledger: Ledger::open(ledger_path.as_ref())?,
            egress,
            counts: Arc::new(RefusalCounts::default()),
            #[cfg(feature = "envelope")]
            evidence: None,
        })
    }

    /// Record every settled outcome, every probe and every compensation in `journal`, as the node `via`
    /// (enforcement point `destination`): an execution record with the receipt's rung, and the
    /// compensation's with the compensated record's hash as its parent.
    #[cfg(feature = "envelope")]
    pub fn with_evidence(mut self, journal: Arc<mycelium::EvidenceJournal>, via: mycelium::NodeId) -> Self {
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
        mycelium::sim_seam::wall_now_ms()
    }

    /// Send `ext` through the `Input` seam: made for real in a recording or a live run, **not made** in a
    /// replay, which returns the recorded answer.
    fn send_through_seam(&self, seam: &str, ext: &ExternalEffect) -> Result<DestinationCommit, EffectRefusal> {
        let key = self.key(&ext.effect.operation_id);
        let request = format!("{key} {:016x} {}", ext.effect.content_hash, ext.class.tag());
        let answer = mycelium::sim_seam::input(seam, &request, || encode_send(&self.destination.send(&key, &ext.effect)));
        decode_send(&answer, &ext.effect.operation_id)
    }

    /// Apply `ext` once. The record is prepared (on disk) before the destination is reached; the
    /// composition, when carried, is checked against `authority` first, and refused without one.
    ///
    /// A second apply of an operation already in the ledger does not reach the destination: a committed
    /// record answers `Replayed` from the ledger; an unknown one is refused `RetryWithoutProbe`; a failed one
    /// starts clean; the rest are refused by state.
    pub async fn apply(&self, ext: &ExternalEffect, authority: Option<&ResourceAuthority>) -> Result<DestinationCommit, ExternalRefusal> {
        self.gate_egress()?;
        let now_ms = Self::now_ms();
        if let Some(composition) = &ext.composition {
            let Some(authority) = authority else {
                let r = EffectRefusal::Unauthorised {
                    leg: crate::CompositionLeg::Authority,
                    reason: "a composed effect needs the resource authority to check it against; none was given".into(),
                };
                self.counts.record(&r);
                return Err(ExternalRefusal::Effect(r));
            };
            if let Err(r) = check_composition(composition, authority, now_ms) {
                self.counts.record(&r);
                return Err(ExternalRefusal::Effect(r));
            }
        }
        let scope = self.destination.dedup_scope().to_string();
        let record_hash = prepared_record_hash(ext, &scope);
        let (record, fresh) = self.ledger.prepare(ext, &scope, now_ms, &record_hash)?;
        if !fresh {
            match record.state {
                EffectState::Committed => return Ok(DestinationCommit::new(self.destination.identity(), DedupOutcome::Replayed)),
                EffectState::Unknown => return Err(self.refuse(ExternalRefusal::RetryWithoutProbe)),
                EffectState::Unresolved => return Err(self.refuse(ExternalRefusal::Unresolved)),
                EffectState::Failed | EffectState::Prepared => {}
                state => return Err(self.refuse(ExternalRefusal::State { state, needed: "a record not yet sent, or one that failed" })),
            }
        }
        self.send_and_settle("effects/apply", ext, None).await
    }

    async fn send_and_settle(&self, seam: &str, ext: &ExternalEffect, parent: Option<String>) -> Result<DestinationCommit, ExternalRefusal> {
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
                self.ledger.set_state(op, EffectState::Committed, None)?;
                self.counts.record_commit();
                let reason = format!("committed at {}: {}", commit.destination, dedup_tag(commit.dedup));
                self.evidence(ext, operation, &reason, Exec::Completed, parent, now_ms).await?;
            }
            Err(EffectRefusal::DeliveryUnknown) => {
                self.ledger.set_state(op, EffectState::Unknown, Some(now_ms))?;
                self.counts.record(&EffectRefusal::DeliveryUnknown);
                self.evidence(ext, operation, "the destination did not answer; the effect's fate is unknown", Exec::Attempted, parent, now_ms).await?;
            }
            Err(e) => {
                self.ledger.set_state(op, EffectState::Failed, None)?;
                self.counts.record(e);
                self.evidence(ext, operation, &e.to_string(), Exec::Failed, parent, now_ms).await?;
            }
        }
        answer.map_err(ExternalRefusal::Effect)
    }

    /// Ask the destination what became of `op`. Only an `Unknown` record is probed; an irreversible class
    /// is refused and stays unknown; past the budget the record is marked `Unresolved` and escalated.
    /// `Done` settles the record as committed; `NotDone` opens one retry where the class allows.
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
        if record.probes >= budget.max_probes || age_ms >= budget.max_age_ms {
            self.ledger.set_state(op, EffectState::Unresolved, None)?;
            let ext = self.ext_of(&record);
            self.evidence(&ext, "effect.probe", &format!("unresolved after {} probes and {age_ms} ms: escalated", record.probes), Exec::Unresolved, Some(record.evidence_hash.clone().unwrap_or(record.record_hash.clone())), now_ms).await?;
            return Err(self.refuse(ExternalRefusal::ProbeBudgetExhausted { probes: record.probes, age_ms }));
        }
        self.gate_egress()?;
        let key = self.key(op);
        let request = format!("{key} probe {}", record.probes + 1);
        let answer = ProbeAnswer::parse(&mycelium::sim_seam::input("effects/probe", &request, || self.destination.observe(&key).tag().to_string()));
        self.ledger.bump(op, "probes")?;
        let ext = self.ext_of(&record);
        let parent = Some(record.evidence_hash.clone().unwrap_or(record.record_hash.clone()));
        match answer {
            ProbeAnswer::Done => {
                self.ledger.set_state(op, EffectState::Committed, None)?;
                self.counts.record_commit();
                self.evidence(&ext, "effect.probe", "the destination reports the effect done: settled by probe", Exec::Completed, parent, now_ms).await?;
            }
            ProbeAnswer::NotDone => {
                self.ledger.set_state(op, EffectState::NotDone, None)?;
                self.evidence(&ext, "effect.probe", "the destination reports the effect not done", Exec::None, parent, now_ms).await?;
            }
            ProbeAnswer::Unknown => {
                self.evidence(&ext, "effect.probe", "the destination could not say", Exec::Unknown, parent, now_ms).await?;
            }
        }
        Ok(answer)
    }

    /// Retry `ext` once, after a probe answered `NotDone`, where the class is idempotent. Refused by name
    /// otherwise: without a probe, a second time, or against a destination not declared idempotent.
    pub async fn retry(&self, ext: &ExternalEffect) -> Result<DestinationCommit, ExternalRefusal> {
        let op = &ext.effect.operation_id;
        let record = self.ledger.get(op)?.ok_or(ExternalRefusal::NotRecorded)?;
        if !matches!(record.class, EffectClass::Idempotent { .. }) {
            return Err(self.refuse(ExternalRefusal::RetryNotIdempotent { class: record.class.tag() }));
        }
        match record.state {
            EffectState::NotDone => {}
            EffectState::Unknown => return Err(self.refuse(ExternalRefusal::RetryWithoutProbe)),
            EffectState::Unresolved => return Err(self.refuse(ExternalRefusal::Unresolved)),
            state => return Err(self.refuse(ExternalRefusal::State { state, needed: "a probe that answered not done" })),
        }
        if record.retries >= 1 {
            return Err(self.refuse(ExternalRefusal::SecondRetry));
        }
        self.gate_egress()?;
        self.ledger.bump(op, "retries")?;
        let parent = Some(record.evidence_hash.clone().unwrap_or(record.record_hash.clone()));
        self.send_and_settle("effects/retry", ext, parent).await
    }

    /// Commit `compensation` for the committed effect `op`, under `permit`. A compensation is a new effect
    /// in the same scope under its own operation id, recorded with the compensated record's hash as its
    /// parent; the compensated record becomes `Compensated`. Refused by name when the class declares no
    /// compensation, when no permit is presented, or when `op` is not committed.
    pub async fn compensate(
        &self,
        op: &OperationId,
        compensation: &ExternalEffect,
        permit: Option<&CompensationPermit>,
    ) -> Result<DestinationCommit, ExternalRefusal> {
        let record = self.ledger.get(op)?.ok_or(ExternalRefusal::NotRecorded)?;
        if record.class.compensation().is_none() {
            return Err(self.refuse(ExternalRefusal::NoCompensation { class: record.class.tag() }));
        }
        let Some(permit) = permit else {
            return Err(self.refuse(ExternalRefusal::CompensationNotPermitted("no evaluator permit was presented".into())));
        };
        if record.state != EffectState::Committed {
            return Err(self.refuse(ExternalRefusal::State { state: record.state, needed: "a committed effect" }));
        }
        if compensation.effect.operation_id == *op {
            return Err(self.refuse(ExternalRefusal::State { state: record.state, needed: "a compensation under its own operation id" }));
        }
        self.gate_egress()?;
        let parent = record.evidence_hash.clone().unwrap_or(record.record_hash.clone());
        let now_ms = Self::now_ms();
        self.record_permit(permit, now_ms).await?;
        let scope = self.destination.dedup_scope().to_string();
        let record_hash = prepared_record_hash(compensation, &scope);
        let (existing, fresh) = self.ledger.prepare(compensation, &scope, now_ms, &record_hash)?;
        if !fresh && existing.state != EffectState::Failed && existing.state != EffectState::Prepared {
            return Err(self.refuse(ExternalRefusal::State { state: existing.state, needed: "a compensation not yet sent" }));
        }
        let commit = self.send_and_settle("effects/compensate", compensation, Some(parent)).await?;
        self.ledger.set_compensated(op, &compensation.effect.operation_id)?;
        Ok(commit)
    }

    fn budget_of(&self, op: &OperationId) -> Result<ProbeBudget, ExternalRefusal> {
        let conn = self.ledger.connect()?;
        conn.query_row(
            "SELECT max_probes, max_age_ms FROM external_effects WHERE operation_id = ?1",
            params![op.as_str()],
            |r| Ok(ProbeBudget { max_probes: r.get::<_, i64>(0)? as u32, max_age_ms: r.get::<_, i64>(1)? as u64 }),
        )
        .map_err(ledger)
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

    #[cfg(feature = "envelope")]
    async fn record_permit(&self, permit: &CompensationPermit, _now_ms: u64) -> Result<(), ExternalRefusal> {
        let Some((journal, _)) = &self.evidence else { return Ok(()) };
        let mut ev = mycelium::AeEvidence::for_decision(&permit.envelope, &permit.decision, mycelium::Execution::Attempted, "destination");
        ev.destination = Some(self.destination.identity().to_string());
        let bytes = serde_json::to_vec(&ev).map_err(ledger)?;
        journal.append(bytes).await.map_err(|e| ExternalRefusal::Ledger(format!("the evidence journal: {e}")))?;
        Ok(())
    }
    #[cfg(not(feature = "envelope"))]
    async fn record_permit(&self, _permit: &CompensationPermit, _now_ms: u64) -> Result<(), ExternalRefusal> {
        Ok(())
    }

    /// Write the execution record for `ext` when a journal is attached, and remember its hash on the
    /// ledger. Without a journal the ledger's own record hash stands, and nothing is written.
    #[cfg(feature = "envelope")]
    async fn evidence(&self, ext: &ExternalEffect, operation: &str, reason: &str, execution: Exec, parent: Option<String>, now_ms: u64) -> Result<(), ExternalRefusal> {
        let Some((journal, via)) = &self.evidence else { return Ok(()) };
        let actor = ext.composition.as_ref().map(|c| c.principal.as_str().to_string()).unwrap_or_else(|| format!("destination:{}", self.destination.identity()));
        let resource = format!("destination:{}", self.destination.identity());
        let envelope = mycelium::ActionEnvelope::builder(actor, via.clone(), operation, &resource)
            .identities(ext.effect.operation_id.to_string(), ext.effect.attempt_id.to_string())
            .validity(now_ms, now_ms.saturating_add(60_000))
            .causal(mycelium::causal::CausalFields { correlation: None, parent })
            .build();
        let decision = mycelium::Decision::permit(reason, "-");
        let mut ev = mycelium::AeEvidence::for_decision(&envelope, &decision, execution.into(), "destination").as_execution(execution.into());
        ev.destination = Some(self.destination.identity().to_string());
        if let Some(c) = &ext.composition
            && let Some(d) = &c.origin_domain
        {
            ev = ev.with_origin_domain(d.clone());
        }
        let bytes = serde_json::to_vec(&ev).map_err(ledger)?;
        let appended = journal.append(bytes).await.map_err(|e| ExternalRefusal::Ledger(format!("the evidence journal: {e}")))?;
        self.ledger.set_evidence_hash(&ext.effect.operation_id, &hex(&appended.content_hash))?;
        Ok(())
    }
    #[cfg(not(feature = "envelope"))]
    async fn evidence(&self, _ext: &ExternalEffect, _operation: &str, _reason: &str, _execution: Exec, _parent: Option<String>, _now_ms: u64) -> Result<(), ExternalRefusal> {
        Ok(())
    }
}

/// The execution states this module writes, named here so the no-`envelope` build has them too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Exec {
    Completed,
    Attempted,
    Failed,
    Unknown,
    None,
    Unresolved,
}

#[cfg(feature = "envelope")]
impl From<Exec> for mycelium::Execution {
    fn from(e: Exec) -> Self {
        match e {
            Exec::Completed => Self::Completed,
            Exec::Attempted => Self::Attempted,
            Exec::Failed => Self::Failed,
            Exec::Unknown => Self::Unknown,
            Exec::None => Self::None,
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
            Err(EffectRefusal::Unauthorised { leg: crate::CompositionLeg::Attribution, reason: "not the holder".into() }),
        ];
        for a in answers {
            assert_eq!(decode_send(&encode_send(&a), &op), a, "{}", encode_send(&a));
        }
        assert!(matches!(decode_send("gibberish", &op), Err(EffectRefusal::Failed(_))), "an unreadable answer is not a commit");
        for p in [ProbeAnswer::Done, ProbeAnswer::NotDone, ProbeAnswer::Unknown] {
            assert_eq!(ProbeAnswer::parse(p.tag()), p);
        }
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
