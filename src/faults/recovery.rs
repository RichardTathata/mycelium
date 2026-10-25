//! The `recovery` class of `mycelium test --faults` — **selective recovery**, the adopter-surface plan's
//! decisive demonstration (X9): the rows compose.
//!
//! Three nodes of the declaration's fleet, each with a reconciler over a ledger of its own, an evidence
//! journal and the evaluator, all three sending to **one mock destination** in this process that loses answers
//! on demand. One effect's acknowledgement is lost (the destination did it; the node cannot know). Dependent
//! work starts on the other two nodes **carrying the parent** — each dependent effect's record names the
//! record it depends on, one of them two steps down — beside independent work that names nothing. The node
//! that lost the answer is cut and restarted over its WAL; a second node goes dark. The fleet then:
//!
//! 1. **probes before it retries**: the restarted node finds its record *unknown* in its own ledger, a retry
//!    without a probe is refused by name, the probe finds the effect done, and the destination holds **one**;
//! 2. **reads the closure** of the unknown record from the journals it can reach — two of three while the
//!    second node is dark, and the reader says so (*INCOMPLETE*, the plan's withheld-journal plant); every
//!    dependent record once the node is back, and no independent one;
//! 3. **recovers each member by its declared class, each as a new record**: an idempotent dependent is probed
//!    and *resumed* by one retry (of two concurrent retries, one is refused before anything is sent); a
//!    compensatable dependent is probed and never retried; a committed dependent whose premise did not happen
//!    is *compensated* under the evaluator's permit; an irreversible one is *left* — unresolved, escalated;
//! 4. **explains** every recovery decision from the records: the class renders `mycelium explain --closure`'s
//!    view over the three journals and keeps it beside the run (`--keep`), and this report names the guarantee
//!    and rule ids the class drove.
//!
//! **Two forms, one set of assertions** (rev 0.8). This is the live form: the published binary, no seams, the
//! destination reached for real. Under the `sim` build the same run records its recovery phases under the
//! replay kernel — the reconcilers' sends and probes are `Input`-class choices — and replays them with the
//! destination **reached zero times in those phases** — the seven sends that set the scene are made live in both
//! forms (`tests/faults_recovery_replay.rs`, and the recording in the corpus).
//!
//! **What it is not.** One destination, mocked, in this process; a closure read offline from files, not at
//! runtime; no policy over the closure's size (the plan's §6). The recovery rule the class applies is the
//! side-effect contract's own (`mycelium::effects::external`): *unknown + idempotent → probe, then retry if
//! not done; unknown + compensatable → probe, never retry; committed on a premise that did not happen →
//! compensate; unknown + irreversible → leave.*

use super::*;

#[cfg(all(feature = "gateway", feature = "tls"))]
use crate::effects::{
    Effect, EffectClass, EffectRefusal, ExternalDestination, ExternalEffect, ExternalRefusal, IdempotencyKey,
    ProbeAnswer, ProbeBudget, Reconciler,
};
#[cfg(all(feature = "gateway", feature = "tls"))]
use crate::{AttemptId, DedupOutcome, DestinationCommit, OperationId};

/// The guarantees the class drives: the crash and restart over the node's own WAL. The compensation's permit is
/// recorded before the compensation is sent — the reconciler's own ordering (`record_permit`), noted in the report —
/// and `ae.recorded_before_dispatch` is the gateway's guarantee, which a fleet with no gateway resolves
/// `not_applicable`, so it is not listed (#677's review, finding 7).
pub(super) const GUARANTEES: &[&str] = &["persist.configured", "persist.sync_mode"];
pub(super) const RULES: &[&str] = &["ae.preflight"];

/// The seamed form's inputs: a kernel (recording or replaying) and the seed its sources take.
#[cfg(feature = "sim")]
pub struct Seamed {
    pub kernel: mycelium_sim::Kernel,
    pub seed: u64,
    /// `true` when the kernel replays: the destination is not reached in the recovery phases, so the checks
    /// that read the destination's counters read the recorded answers instead and say so.
    pub replaying: bool,
}

/// What the seamed form hands back beside the class report.
#[cfg(feature = "sim")]
pub struct SeamedOutcome {
    pub trace: mycelium_sim::Trace,
    pub exhausted: bool,
    pub position: usize,
    /// How many times the destination was reached during the two recovery phases: the recording's count, or
    /// zero in a faithful replay.
    pub destination_reached_in_recovery: usize,
}

#[cfg(not(all(feature = "gateway", feature = "tls")))]
pub(super) fn run(rep: &mut ClassReport, base: &GossipConfig, _units: &[Unit], _dir: &Path) {
    // The feature the build lacks, by name: `tls` when the gateway is in, `gateway` otherwise (the sibling
    // `policy-diff` does the same; #677's review, finding 6).
    rep.state = ClassState::NotInBuild { feature: if cfg!(feature = "gateway") { "tls" } else { "gateway" } };
    let why = class_reason(&rep.state);
    not_exercised(rep, unstarted_report(base).as_ref(), GUARANTEES, RULES, &why);
}

#[cfg(all(feature = "gateway", feature = "tls"))]
pub(super) fn run(rep: &mut ClassReport, base: &GossipConfig, units: &[Unit], dir: &Path) {
    #[cfg(feature = "sim")]
    let _ = run_inner(rep, base, units, dir, None);
    #[cfg(not(feature = "sim"))]
    let _ = run_inner(rep, base, units, dir);
}

/// The seamed form (`sim` only): the recovery phases under `seamed.kernel`.
#[cfg(all(feature = "gateway", feature = "tls", feature = "sim"))]
pub(super) fn run_seamed(rep: &mut ClassReport, base: &GossipConfig, units: &[Unit], dir: &Path, seamed: Seamed) -> Result<SeamedOutcome, String> {
    run_inner(rep, base, units, dir, Some(seamed)).ok_or_else(|| "the class did not reach its recovery phases; its report says why".to_string())
}

// ── The mock destination ─────────────────────────────────────────────────────────────────────────

/// One key's history at the destination.
#[cfg(all(feature = "gateway", feature = "tls"))]
#[derive(Default)]
struct Entry {
    /// The content hash committed, when the effect was applied.
    hash: Option<u64>,
    /// How many sends reached it.
    attempts: u32,
    /// How many times it was applied — the number the plan's *one effect* is about.
    applications: u32,
}

/// The destination outside the fleet, in this process: applies effects under an idempotency key and **loses
/// answers on demand**, once per key. A payload beginning `lose:` is applied on its first send and not answered
/// (the destination did it; the caller cannot know); one beginning `vanish:` is neither applied nor answered on
/// its first send (the blip caught it before it landed); a later send of either behaves normally. Every send and
/// probe is counted, so the class can say how many times the destination was reached. Its map is a leaf lock
/// (lock-order table row 69), held for one update.
#[cfg(all(feature = "gateway", feature = "tls"))]
struct Lossy {
    entries: std::sync::Mutex<std::collections::HashMap<String, Entry>>,
    reached: std::sync::atomic::AtomicUsize,
}

#[cfg(all(feature = "gateway", feature = "tls"))]
impl Lossy {
    const IDENTITY: &'static str = "mock-destination";

    fn new() -> Arc<Self> {
        Arc::new(Self { entries: std::sync::Mutex::new(Default::default()), reached: std::sync::atomic::AtomicUsize::new(0) })
    }

    /// Every send and probe that reached the destination — the number the seamed form's replay holds at zero.
    #[cfg(feature = "sim")]
    fn reached(&self) -> usize {
        self.reached.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn applications(&self, key: &IdempotencyKey) -> u32 {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).get(&key.to_string()).map(|e| e.applications).unwrap_or(0)
    }

    /// How many sends reached the destination under `key` — the number a re-send shows, where `applications` stays
    /// at one because the destination deduplicates (#677's review, finding 4).
    fn attempts(&self, key: &IdempotencyKey) -> u32 {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).get(&key.to_string()).map(|e| e.attempts).unwrap_or(0)
    }

    fn total_applications(&self) -> u32 {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).values().map(|e| e.applications).sum()
    }
}

#[cfg(all(feature = "gateway", feature = "tls"))]
impl ExternalDestination for Lossy {
    fn identity(&self) -> &str {
        Self::IDENTITY
    }
    fn dedup_scope(&self) -> &str {
        Self::IDENTITY
    }
    fn host(&self) -> Option<&str> {
        None
    }
    fn send(&self, key: &IdempotencyKey, effect: &Effect) -> Result<DestinationCommit, EffectRefusal> {
        self.reached.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let text = String::from_utf8_lossy(&effect.payload).into_owned();
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = map.entry(key.to_string()).or_default();
        entry.attempts += 1;
        let first = entry.attempts == 1;
        if first && text.starts_with("vanish:") {
            return Err(EffectRefusal::DeliveryUnknown);
        }
        let outcome = match entry.hash {
            Some(h) if h == effect.content_hash => DedupOutcome::Replayed,
            Some(h) => return Err(EffectRefusal::Conflict { operation_id: effect.operation_id.clone(), committed_hash: h, presented_hash: effect.content_hash }),
            None => {
                entry.hash = Some(effect.content_hash);
                entry.applications += 1;
                DedupOutcome::Fresh
            }
        };
        if first && text.starts_with("lose:") {
            return Err(EffectRefusal::DeliveryUnknown);
        }
        Ok(DestinationCommit::new(Self::IDENTITY, outcome))
    }
    fn observe(&self, key: &IdempotencyKey) -> ProbeAnswer {
        self.reached.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match self.entries.lock().unwrap_or_else(|e| e.into_inner()).get(&key.to_string()).and_then(|e| e.hash) {
            Some(_) => ProbeAnswer::Done,
            None => ProbeAnswer::NotDone,
        }
    }
}

// ── The run ──────────────────────────────────────────────────────────────────────────────────────

/// The evidence journal opened on a node's runtime before `start()`, handed back out of the start closure.
#[cfg(all(feature = "gateway", feature = "tls"))]
type Opened = Arc<std::sync::Mutex<Option<Result<Arc<crate::EvidenceJournal>, String>>>>;

/// Start node `i` with its journal (at `journal_path(dir, i)`) and the evaluator attached before the lifecycle
/// boundary; the journal's writer is spawned on the node's runtime, where the reconciler will run too.
#[cfg(all(feature = "gateway", feature = "tls"))]
fn start_node(fleet: &mut Fleet<'_>, i: usize, dir: &Path, evaluator: &Arc<dyn crate::ActionEvaluator>) -> Result<Arc<crate::EvidenceJournal>, String> {
    let opened: Opened = Arc::new(std::sync::Mutex::new(None));
    let (ev, slot, path) = (Arc::clone(evaluator), Arc::clone(&opened), journal_path(dir, i));
    fleet.start_with(i, move |a| {
        a.with_action_evaluator(ev);
        let j = crate::EvidenceJournal::open(&path, crate::EvidenceProfile::Strict).map_err(|e| format!("opening {}: {e}", path.display()));
        if let Ok(j) = &j {
            a.with_evidence_journal(Arc::clone(j));
        }
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(j);
    })?;
    match opened.lock().unwrap_or_else(|e| e.into_inner()).take() {
        Some(Ok(j)) => Ok(j),
        Some(Err(reason)) => Err(reason),
        None => Err("the journal was not opened".into()),
    }
}

#[cfg(all(feature = "gateway", feature = "tls"))]
fn journal_path(dir: &Path, i: usize) -> PathBuf {
    dir.join(format!("journal-{i}")).join("evidence.log")
}

#[cfg(all(feature = "gateway", feature = "tls"))]
fn ledger_path(dir: &Path, i: usize) -> PathBuf {
    dir.join(format!("ledger-{i}.journal"))
}

/// Open node `i`'s reconciler on its runtime, writing evidence to `journal` as node `i`.
#[cfg(all(feature = "gateway", feature = "tls"))]
fn open_reconciler(fleet: &Fleet<'_>, i: usize, dir: &Path, dest: &Arc<Lossy>, journal: &Arc<crate::EvidenceJournal>) -> Result<Reconciler<Lossy>, String> {
    let (path, dest, journal, via) = (ledger_path(dir, i), Arc::clone(dest), Arc::clone(journal), fleet.nodes[i].id.clone());
    fleet.on(i, async move { Reconciler::open(&path, dest, crate::EgressPolicy::default()).map(|r| r.with_evidence(journal, via)).map_err(|e| format!("node {i}'s reconciler: {e}")) })
}

/// An effect of the demonstration: its operation, payload (the mock's prefixes say what the blip does to it), class.
#[cfg(all(feature = "gateway", feature = "tls"))]
fn effect(op: &str, payload: &str, class: EffectClass) -> ExternalEffect {
    let op = OperationId::new(op);
    ExternalEffect {
        effect: Effect::new(op.clone(), AttemptId::of(&op, 1), payload.as_bytes().to_vec()),
        class,
        probe_budget: ProbeBudget { max_probes: 3, max_age_ms: 600_000 },
        composition: None,
    }
}

/// The hash of an effect's latest evidence record, as its ledger holds it.
#[cfg(all(feature = "gateway", feature = "tls"))]
fn hash_of(r: &Reconciler<Lossy>, ext: &ExternalEffect) -> Result<String, String> {
    r.record(&ext.effect.operation_id)
        .map_err(|e| e.to_string())?
        .and_then(|rec| rec.evidence_hash)
        .ok_or_else(|| format!("{} has no evidence record on its ledger", ext.effect.operation_id))
}

#[cfg(all(feature = "gateway", feature = "tls"))]
fn state_of(r: &Reconciler<Lossy>, ext: &ExternalEffect) -> String {
    r.record(&ext.effect.operation_id).ok().flatten().map(|rec| rec.state.to_string()).unwrap_or_else(|| "not recorded".into())
}

#[cfg(all(feature = "gateway", feature = "tls"))]
fn short(h: &str) -> &str {
    &h[..12.min(h.len())]
}

/// The whole run. Returns the seamed outcome when a kernel was given and the recovery phases ran.
#[cfg(all(feature = "gateway", feature = "tls"))]
fn run_inner(
    rep: &mut ClassReport,
    base: &GossipConfig,
    units: &[Unit],
    dir: &Path,
    #[cfg(feature = "sim")] seamed: Option<Seamed>,
) -> Option<SeamedOutcomeOrUnit> {
    use crate::causal::CausalFields;
    use crate::effects::CompensationPermit;

    if base.persistence.is_none() {
        rep.state = ClassState::NotConfigured { missing: "[persistence]".into() };
        let why = class_reason(&rep.state);
        not_exercised(rep, unstarted_report(base).as_ref(), GUARANTEES, RULES, &why);
        return None;
    }
    let mut fleet = match Fleet::plan(base, units, dir, units.len().max(3)) {
        Ok(f) => f,
        Err(reason) => { rep.state = ClassState::Refused { reason }; return None; }
    };
    // One evaluator for the fleet: it decides the compensation (`ae.preflight`, recorded before the compensation
    // is sent), as it would decide any action; nothing else here is a policy decision.
    const ACTOR: &str = "test-faults";
    const RETRACT: &str = "notify-retract";
    let evaluator: Arc<dyn crate::ActionEvaluator> = Arc::new(
        crate::ReferenceEvaluator::new("test-faults")
            .with_catalogue("recovery", "1")
            .map_action(RETRACT, "*")
            .allow(crate::Rule::new(ACTOR, RETRACT, format!("destination:{}", Lossy::IDENTITY))),
    );
    let mut journals: Vec<Option<Arc<crate::EvidenceJournal>>> = Vec::new();
    for i in 0..fleet.len() {
        match start_node(&mut fleet, i, dir, &evaluator) {
            Ok(j) => journals.push(Some(j)),
            Err(reason) => { rep.state = ClassState::Refused { reason }; return None; }
        }
    }
    exercised(rep, fleet.guarantees().as_ref(), GUARANTEES, RULES);
    let dest = Lossy::new();
    // The demonstration uses the fleet's first three nodes; a declaration with more units adds nodes that gossip
    // and hold no reconciler.
    let mut recs: Vec<Option<Reconciler<Lossy>>> = Vec::new();
    for (i, journal) in journals.iter().take(3).enumerate() {
        match open_reconciler(&fleet, i, dir, &dest, journal.as_ref().expect("opened")) {
            Ok(r) => recs.push(Some(r)),
            Err(reason) => { rep.state = ClassState::Refused { reason }; return None; }
        }
    }
    let refused = |rep: &mut ClassReport, reason: String| {
        rep.state = ClassState::Refused { reason };
    };

    // ── Phase A, unseamed: the effects, the dependents, the independents, the crash and the dark node ──
    let correlation = "order-100-run".to_string();
    let root = effect("order-100", "lose:order-100", EffectClass::Idempotent { compensation: None });
    let d1 = effect("label-101", "vanish:label-101", EffectClass::Idempotent { compensation: None });
    let d2 = effect("points-102", "vanish:points-102", EffectClass::Compensatable { compensation: "points-refund".into() });
    let d3 = effect("notify-103", "notify-103", EffectClass::Compensatable { compensation: RETRACT.into() });
    let d4 = effect("email-104", "vanish:email-104", EffectClass::Irreversible);
    let i1 = effect("audit-201", "audit-201", EffectClass::Idempotent { compensation: None });
    let i2 = effect("report-202", "report-202", EffectClass::Idempotent { compensation: None });
    let caused = |parent: &str| CausalFields { correlation: Some(correlation.clone()), parent: Some(parent.to_string()) };

    let r0 = recs[0].as_ref().expect("node 0's reconciler");
    let lost = fleet.on(0, r0.apply_caused(&root, None, CausalFields { correlation: Some(correlation.clone()), parent: None }));
    if !matches!(lost, Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown))) {
        refused(rep, format!("the mock destination did not lose the root effect's answer: {lost:?}"));
        return None;
    }
    let h0 = match hash_of(r0, &root) { Ok(h) => h, Err(e) => { refused(rep, e); return None; } };
    let r1 = recs[1].as_ref().expect("node 1's reconciler");
    let r2 = recs[2].as_ref().expect("node 2's reconciler");
    let a_d1 = fleet.on(1, r1.apply_caused(&d1, None, caused(&h0)));
    let a_d2 = fleet.on(2, r2.apply_caused(&d2, None, caused(&h0)));
    let h_d2 = match hash_of(r2, &d2) { Ok(h) => h, Err(e) => { refused(rep, e); return None; } };
    let a_d3 = fleet.on(1, r1.apply_caused(&d3, None, caused(&h_d2)));
    let a_d4 = fleet.on(2, r2.apply_caused(&d4, None, caused(&h0)));
    let a_i1 = fleet.on(1, r1.apply_caused(&i1, None, CausalFields { correlation: Some("audit-run".into()), parent: None }));
    let a_i2 = fleet.on(2, r2.apply_caused(&i2, None, CausalFields { correlation: Some("report-run".into()), parent: None }));
    let unknown = |r: &Result<DestinationCommit, ExternalRefusal>| matches!(r, Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown)));
    if !(unknown(&a_d1) && unknown(&a_d2) && a_d3.is_ok() && unknown(&a_d4) && a_i1.is_ok() && a_i2.is_ok()) {
        refused(rep, format!("the dependents did not land as the blip intends: d1 {a_d1:?}, d2 {a_d2:?}, d3 {a_d3:?}, d4 {a_d4:?}, i1 {a_i1:?}, i2 {a_i2:?}"));
        return None;
    }
    let (h_d1, h_d3, h_d4, h_i1, h_i2) = match (hash_of(r1, &d1), hash_of(r1, &d3), hash_of(r2, &d4), hash_of(r1, &i1), hash_of(r2, &i2)) {
        (Ok(a), Ok(b), Ok(c), Ok(d), Ok(e)) => (a, b, c, d, e),
        _ => { refused(rep, "a dependent has no evidence record".into()); return None; }
    };
    let applied_before = dest.total_applications();

    // A synced write on node 0 before the crash: the restart's WAL replay is asserted, not assumed (#677's review).
    let marker_key = "faults/recovery/marker".to_string();
    let marker = {
        let (agent, key) = (fleet.agent(0), marker_key.clone());
        fleet.on(0, async move { agent.kv().set_requiring_sync(&OperationId::new("test-faults-recovery-marker"), key, b"before-crash".to_vec()).await })
    };
    if let Err(e) = marker {
        refused(rep, format!("node 0's synced write before the crash: {e}"));
        return None;
    }

    // The crash: node 0's runtime dropped with its reconciler's writer and its journal's; restarted over its own
    // directory, its WAL, its journal and its ledger re-opened from disk.
    recs[0] = None;
    journals[0] = None;
    fleet.cut(0);
    match start_node(&mut fleet, 0, dir, &evaluator) {
        Ok(j) => journals[0] = Some(j),
        Err(reason) => { refused(rep, format!("node 0 did not restart over its own directory: {reason}")); return None; }
    }
    match open_reconciler(&fleet, 0, dir, &dest, journals[0].as_ref().expect("re-opened")) {
        Ok(r) => recs[0] = Some(r),
        Err(reason) => { refused(rep, reason); return None; }
    }
    // The partition: node 2 goes dark, and node 0 sees it leave.
    let id2 = fleet.nodes[2].id.clone();
    recs[2] = None;
    journals[2] = None;
    fleet.cut(2);
    let evicted = fleet.wait(0, Duration::from_secs(30), || !fleet.agent(0).peers().contains(&id2));
    let wal_back = fleet.agent(0).kv().get(&marker_key).is_some_and(|v| v.as_ref() == b"before-crash");
    rep.notes.push(format!("root record {h0} (`mycelium explain --closure {h0} <dir>/journal-*/evidence.log --fleet 3` over a kept run); the compensation's permit is recorded before the compensation is sent (the reconciler's `record_permit`), which is this class's ordering and not the gateway's `ae.recorded_before_dispatch`"));

    // ── Phase B, seamed: recovery over the journals the fleet can reach ──
    // The kernel, when one was given, sees the recovery phases only: the fleet's own waits and the nodes'
    // runtimes are outside it, so what is recorded is what the reconcilers asked the destination and the clock.
    #[cfg(feature = "sim")]
    let (mut ctx, replaying) = match seamed {
        Some(s) => (
            Some(crate::sim_seam::SimContext { kernel: s.kernel, sources: mycelium_sim::Sources::seeded(s.seed, 1_700_000_000_000), node: "recovery".into(), offsets: Default::default() }),
            s.replaying,
        ),
        None => (None, false),
    };
    #[cfg(not(feature = "sim"))]
    let replaying = false;
    #[cfg(feature = "sim")]
    let reached_before_recovery = dest.reached();
    #[cfg(feature = "sim")]
    if let Some(c) = ctx.take() {
        crate::sim_seam::install(c);
    }

    let r0 = recs[0].as_ref().expect("node 0's reconciler, re-opened");
    let r1 = recs[1].as_ref().expect("node 1's reconciler");
    // 1. Probe before retry; the destination holds one.
    let t = crate::sim_seam::mono_instant();
    let found = state_of(r0, &root);
    let retry_refused = fleet.on(0, r0.apply(&root, None));
    let probe = fleet.on(0, r0.probe(&root.effect.operation_id));
    let settled = state_of(r0, &root);
    // The destination deduplicates, so `applications` stays at one whatever the reconciler does; `attempts` is the
    // number a re-send shows (#677's review, finding 4).
    let root_key = IdempotencyKey { scope: Lossy::IDENTITY.into(), operation_id: root.effect.operation_id.clone() };
    let (root_attempts, root_applications) = (dest.attempts(&root_key), dest.applications(&root_key));
    let one = replaying || (root_attempts == 1 && root_applications == 1);
    let ok = wal_back && found == "unknown" && matches!(retry_refused, Err(ExternalRefusal::RetryWithoutProbe)) && probe == Ok(ProbeAnswer::Done) && settled == "committed" && one;
    check(rep, &t, "the restarted node finds its lost-acknowledgement record unknown in its own ledger, probes before any retry, and the destination holds one effect", Some(ok),
        format!("restarted over its WAL: the synced write is {}; found {found}; a retry without a probe: {}; probe: {probe:?}; then {settled}; {}", if wal_back { "back" } else { "MISSING" }, match &retry_refused { Err(e) => e.to_string(), Ok(c) => format!("SENT ({c:?})") },
            if replaying { "replay: the destination was not reached in this phase; the recorded answers stand".to_string() } else { format!("the destination reached {root_attempts} time(s) for it, one application") }));

    // 2. The idempotent dependent: probed, resumed by one retry; of two concurrent retries one is refused.
    let t = crate::sim_seam::mono_instant();
    let p1 = fleet.on(1, r1.probe(&d1.effect.operation_id));
    let (ra, rb) = fleet.on(1, async { tokio::join!(r1.retry(&d1, None), r1.retry(&d1, None)) });
    let fresh = [&ra, &rb].iter().filter(|r| matches!(r, Ok(c) if c.dedup == DedupOutcome::Fresh)).count();
    let second = [&ra, &rb].iter().filter(|r| matches!(r, Err(ExternalRefusal::SecondRetry))).count();
    let d1_key = IdempotencyKey { scope: Lossy::IDENTITY.into(), operation_id: d1.effect.operation_id.clone() };
    let one_d1 = replaying || (dest.applications(&d1_key) == 1 && dest.attempts(&d1_key) == 2);
    let ok = p1 == Ok(ProbeAnswer::NotDone) && fresh == 1 && second == 1 && state_of(r1, &d1) == "committed" && one_d1;
    check(rep, &t, "an idempotent dependent whose fate was unknown is probed, found not done, and resumed by one retry — of two concurrent retries one is refused before anything is sent", Some(ok),
        format!("probe: {p1:?}; retries: {} fresh, {} refused as a second retry; then {}", fresh, second, state_of(r1, &d1)));

    // 3. The closure over two of three journals: incomplete, and said so.
    let t = crate::sim_seam::mono_instant();
    let two = [journal_path(dir, 0), journal_path(dir, 1)];
    let (partial_text, partial_members) = match crate::closure::Corpus::load(&two) {
        Ok(c) => (c.closure(&h0, Some(fleet.len())), c.closure_members(&h0)),
        Err(e) => (format!("could not read the journals: {e}"), Vec::new()),
    };
    let has = |members: &[String], h: &str| members.iter().any(|m| m == h);
    // A hole hides what hangs below it: node 1's `notify-103` depends on node 2's `points-102`, so with node 2's
    // journal withheld it is outside the closure too, and the reader names its parent as a hole.
    let fleet_line = partial_text.lines().find(|l| l.starts_with("INCOMPLETE") || l.contains("journal(s) given")).unwrap_or("").trim().to_string();
    let hole_line = partial_text.lines().find(|l| l.contains("hole(s) cited elsewhere")).unwrap_or("").trim().to_string();
    let ok = evicted && partial_text.contains("INCOMPLETE") && has(&partial_members, &h_d1) && !has(&partial_members, &h_d2) && !has(&partial_members, &h_d3) && !has(&partial_members, &h_d4) && partial_text.contains(&h_d2);
    check(rep, &t, "with a node dark — evicted from the restarted node's peer table — the closure read over two of three journals says it is incomplete: node 1's dependent is in it, node 2's are not, nor the record two steps down through them — its parent is named as a hole", Some(ok),
        format!("node 2 {}; {} member(s); {} present: {}; {} absent: {}; {} absent: {} (its parent {} is in the withheld journal); {fleet_line}; {hole_line}", if evicted { "evicted at node 0" } else { "NOT evicted at node 0 after 30 s" }, partial_members.len(),
            short(&h_d1), has(&partial_members, &h_d1), short(&h_d2), !has(&partial_members, &h_d2), short(&h_d3), !has(&partial_members, &h_d3), short(&h_d2)));

    #[cfg(feature = "sim")]
    let mut ctx = crate::sim_seam::take();

    // ── Phase C, unseamed: node 2 returns ──
    match start_node(&mut fleet, 2, dir, &evaluator) {
        Ok(j) => journals[2] = Some(j),
        Err(reason) => { refused(rep, format!("node 2 did not restart over its own directory: {reason}")); return None; }
    }
    match open_reconciler(&fleet, 2, dir, &dest, journals[2].as_ref().expect("re-opened")) {
        Ok(r) => recs[2] = Some(r),
        Err(reason) => { refused(rep, reason); return None; }
    }
    let back = fleet.converge(Duration::from_secs(30));
    rep.notes.push(format!("node 2 back: {}", if back { "every node has every other as a peer again" } else { "not every node had every other as a peer after 30 s (the journals are read from disk either way)" }));

    // ── Phase D, seamed: the rest of the closure, by class ──
    #[cfg(feature = "sim")]
    if let Some(c) = ctx.take() {
        crate::sim_seam::install(c);
    }
    let r1 = recs[1].as_ref().expect("node 1's reconciler");
    let r2 = recs[2].as_ref().expect("node 2's reconciler, re-opened");

    // 4. The closure complete: every dependent, no independent.
    let t = crate::sim_seam::mono_instant();
    let three = [journal_path(dir, 0), journal_path(dir, 1), journal_path(dir, 2)];
    let members = crate::closure::Corpus::load(&three).map(|c| c.closure_members(&h0)).unwrap_or_default();
    let ok = [&h_d1, &h_d2, &h_d3, &h_d4].iter().all(|h| has(&members, h)) && !has(&members, &h_i1) && !has(&members, &h_i2);
    check(rep, &t, "the returned node's journal completes the closure: every dependent record is in it and no independent one", Some(ok),
        format!("{} member(s); dependents {}, {}, {} (two steps down), {}; independents {} and {} outside", members.len(), short(&h_d1), short(&h_d2), short(&h_d3), short(&h_d4), short(&h_i1), short(&h_i2)));

    // 5. The compensatable dependent: probed, not done, never retried.
    let t = crate::sim_seam::mono_instant();
    let p2 = fleet.on(2, r2.probe(&d2.effect.operation_id));
    let retry2 = fleet.on(2, r2.retry(&d2, None));
    let ok = p2 == Ok(ProbeAnswer::NotDone) && matches!(retry2, Err(ExternalRefusal::RetryNotIdempotent { .. })) && state_of(r2, &d2) == "not_done";
    check(rep, &t, "a compensatable dependent whose fate was unknown is probed, found not done, and never retried", Some(ok),
        format!("probe: {p2:?}; a retry: {}; then {}", match &retry2 { Err(e) => e.to_string(), Ok(c) => format!("SENT ({c:?})") }, state_of(r2, &d2)));

    // 6. The committed dependent of an effect that did not happen: compensated under the permit, as a new record.
    let t = crate::sim_seam::mono_instant();
    let retract = effect("notify-103-retract", "retract:notify-103", EffectClass::Idempotent { compensation: None });
    let now_ms = crate::sim_seam::wall_now_ms();
    let via = fleet.nodes[1].id.clone();
    let compensated = match CompensationPermit::decide(&evaluator, ACTOR, via, Lossy::IDENTITY, RETRACT, &h_d3, now_ms) {
        Ok(permit) => fleet.on(1, r1.compensate(&d3.effect.operation_id, &retract, Some(&permit))).map_err(|e| e.to_string()),
        Err(e) => Err(format!("no permit: {e}")),
    };
    let h_retract = hash_of(r1, &retract).ok();
    let after = crate::closure::Corpus::load(&three).ok();
    let retract_in_closure = after.as_ref().zip(h_retract.as_ref()).is_some_and(|(c, h)| c.closure_members(&h0).iter().any(|m| m == h));
    let names_it = after.as_ref().zip(h_retract.as_ref()).and_then(|(c, h)| c.get(h).map(|r| r.parent.as_deref() == Some(h_d3.as_str()))).unwrap_or(false);
    let ok = compensated.is_ok() && state_of(r1, &d3) == "compensated" && retract_in_closure && names_it;
    check(rep, &t, "a committed dependent of an effect that did not happen is compensated under the evaluator's permit, as a new record naming it", Some(ok),
        format!("{}; then {}; the compensation's record {} the compensated record as its parent and is in the closure: {}", match &compensated { Ok(c) => format!("compensated ({:?})", c.dedup), Err(e) => format!("NOT compensated: {e}") }, state_of(r1, &d3), if names_it { "names" } else { "does NOT name" }, retract_in_closure));

    // 7. The irreversible dependent: neither probed nor retried; left, as a new record.
    let t = crate::sim_seam::mono_instant();
    let probe4 = fleet.on(2, r2.probe(&d4.effect.operation_id));
    let left = fleet.on(2, r2.leave(&d4.effect.operation_id));
    let members_after = crate::closure::Corpus::load(&three).ok();
    let leave_record = members_after.as_ref().and_then(|c| c.closure_members(&h0).into_iter().find(|m| c.get(m).is_some_and(|r| r.parent.as_deref() == Some(h_d4.as_str()))));
    let ok = matches!(probe4, Err(ExternalRefusal::IrreversibleUnobservable)) && left.is_ok() && state_of(r2, &d4) == "unresolved" && leave_record.is_some();
    check(rep, &t, "an irreversible dependent is neither probed nor retried; it is left — unresolved, escalated — as a new record naming it", Some(ok),
        format!("a probe: {}; leave: {}; then {}; the leave's record {}", match &probe4 { Err(e) => e.to_string(), Ok(a) => format!("ANSWERED {a:?}") }, match &left { Ok(()) => "recorded".to_string(), Err(e) => e.to_string() }, state_of(r2, &d4), leave_record.as_deref().map(short).map(|s| format!("{s} is in the closure")).unwrap_or_else(|| "is NOT in the closure".into())));

    // 8. Independent work: untouched.
    let t = crate::sim_seam::mono_instant();
    let cites = |c: &crate::closure::Corpus, h: &str| c.closure_members(h).len();
    let (c_i1, c_i2) = members_after.as_ref().map(|c| (cites(c, &h_i1), cites(c, &h_i2))).unwrap_or((0, 0));
    let ok = state_of(r1, &i1) == "committed" && state_of(r2, &i2) == "committed" && c_i1 == 1 && c_i2 == 1 && !has(&members, &h_i1) && !has(&members, &h_i2);
    check(rep, &t, "independent work beside the closure is untouched: its records are outside the closure, its ledger states unchanged, no new record cites it", Some(ok),
        format!("{} {}, {} {}; records citing each: {} and {} (itself only)", i1.effect.operation_id, state_of(r1, &i1), i2.effect.operation_id, state_of(r2, &i2), c_i1 - 1, c_i2 - 1));

    // 9. The destination saw each effect once.
    let t = crate::sim_seam::mono_instant();
    let key = |e: &ExternalEffect| IdempotencyKey { scope: Lossy::IDENTITY.into(), operation_id: e.effect.operation_id.clone() };
    if replaying {
        #[cfg(feature = "sim")]
        {
            let delta = dest.reached() - reached_before_recovery;
            check(rep, &t, "the destination was reached zero times during the recovery's replay: every send and probe answered from the recording", Some(delta == 0), format!("{delta} call(s) reached the destination during the two recovery phases; {applied_before} application(s) stood from before them"));
        }
    } else {
        // Applications and attempts, per key: the root reached once (the lost answer never re-sent), the resumed
        // dependent twice (the vanished send, then the one retry), everything else once.
        let counts = [(&root, 1u32, 1u32), (&d1, 1, 2), (&d2, 0, 1), (&d3, 1, 1), (&d4, 0, 1), (&i1, 1, 1), (&i2, 1, 1), (&retract, 1, 1)]
            .map(|(e, want_applied, want_attempts)| (e.effect.operation_id.to_string(), dest.applications(&key(e)), want_applied, dest.attempts(&key(e)), want_attempts));
        let ok = counts.iter().all(|(_, got, want, got_a, want_a)| got == want && got_a == want_a) && dest.total_applications() == applied_before + 2;
        check(rep, &t, "the destination saw each effect once: the lost acknowledgement never re-sent, the one retry landed once, the compensation once, nothing vanished twice", Some(ok),
            format!("per key, applications then attempts: {}; {} application(s) in all, {applied_before} before the recovery", counts.iter().map(|(op, got, want, got_a, want_a)| format!("{op} {got}/{want} {got_a}/{want_a}")).collect::<Vec<_>>().join(", "), dest.total_applications()));
    }

    // 10. Explained from the records.
    let t = crate::sim_seam::mono_instant();
    let rendering = crate::closure::Corpus::load(&three).map(|c| c.closure(&h0, Some(fleet.len()))).unwrap_or_else(|e| format!("could not read the journals: {e}"));
    // A compensation is recorded under the evaluator's envelope: its operation is the class's declared one.
    let verbs = ["effect.probe", "effect.retry", RETRACT, "effect.leave"];
    let seen: Vec<&str> = verbs.iter().copied().filter(|v| rendering.contains(v)).collect();
    let _ = std::fs::write(dir.join("closure.txt"), &rendering);
    let ok = seen.len() == verbs.len() && !rendering.contains("INCOMPLETE") && rendering.contains(&format!("{} journal(s) given for a fleet of {} nodes", 3, fleet.len()));
    check(rep, &t, "`mycelium explain --closure` renders every recovery decision from the three journals, complete", Some(ok),
        format!("{} record(s) rendered; the probe, the retry, the compensation (under its declared operation) and the leave seen: {}; kept as <dir>/closure.txt", rendering.lines().filter(|l| l.contains("[captured")).count(), if seen.is_empty() { "none".to_string() } else { seen.join(", ") }));
    rep.notes.push("the rule applied, per member: unknown + idempotent → probe, then one retry if not done; unknown + compensatable → probe, never retry (found not done it ends `not_done`: nothing at the destination, a new operation is the caller's); committed on a premise that did not happen → compensate under the evaluator's permit; unknown + irreversible → leave (unresolved, escalated); independent work → nothing".into());

    #[cfg(feature = "sim")]
    let out = crate::sim_seam::take().map(|c| SeamedOutcome { trace: c.kernel.trace().clone(), exhausted: c.kernel.is_exhausted(), position: c.kernel.position(), destination_reached_in_recovery: dest.reached() - reached_before_recovery });
    #[cfg(not(feature = "sim"))]
    let out = None;
    // The reconcilers before the fleet: their ledgers' writers live on the nodes' runtimes.
    drop(recs);
    drop(fleet);
    out
}

#[cfg(all(feature = "gateway", feature = "tls", feature = "sim"))]
type SeamedOutcomeOrUnit = SeamedOutcome;
#[cfg(all(feature = "gateway", feature = "tls", not(feature = "sim")))]
type SeamedOutcomeOrUnit = ();
