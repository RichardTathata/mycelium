//! The node-local evidence journal — AE0 §5's ack-capable contract.
//!
//! # Why this exists, and what it replaces
//!
//! The first implementation of gateway evidence (PR #224) sealed the whole decision document into
//! the tamper-evident audit chain. That chain is an ordinary signed KV entry, so it **gossips to
//! every node**: the exact resource a call targeted, the reason a policy gave, and the constraints
//! it checked were disseminated cluster-wide. AE0 §5 corrected against precisely that shape, after
//! a review of its own first draft, and this module is the correction.
//!
//! Evidence is written **here** — node-local, append-only, fsynced, never gossiped. What enters the
//! chain is a [safe reference record](super::action_evaluator::AeReference) only: the decision's
//! verdict, the policy revision, the identities, the catalogue id, and the **content hash of the
//! journal record**. The chain's hash-linking therefore still covers the evidence — a journal record
//! that does not match its chained hash is detectable — while the evidence itself stays where
//! §6.7's rule puts it: outside gossip KV, leaving only through an exporter.
//!
//! # The mechanism lives in [`journal`](super::journal) (item 4 PR 3a)
//!
//! This module is the AE **profile** over the node-local journal, not the journal. The append-only
//! file, the fsync, the bounded queue, the reader and its cursor moved to `super::journal`, because
//! the rights ledger (`docs/design/adaptive-stability.md` §4) needs the same mechanism and a second
//! journal beside the first is how guarantees drift. The mechanism is ungated — the ledger is its
//! second user, in every build — while this profile keeps its gateway gate. Every public name this
//! module exported before the split is still exported from here, unchanged; the AE journal records
//! its queue under the replay stream `ae/journal`, exactly as before.
//!
//! # Why not the audit sink
//!
//! `AuditSink::export` returns nothing, runs on a drain task, and drops records when its bounded
//! channel saturates. It is a **mirror**, and a mirror cannot be a durability barrier. AE0 names the
//! journal, not the sink, as the contract that can acknowledge — which is why an effect may be
//! gated on this and never on that.
//!
//! # The receipt is item 1's, not a new one
//!
//! [`append`](EvidenceJournal::append) returns
//! [`LocalDurability`](mycelium_core::receipt::LocalDurability): `OnDisk` only after the record is
//! fsynced, `Failed` when durability was not established, `NotConfigured` when no journal is
//! attached. `Buffered` is never produced here — every append forces a sync, because an evidence
//! record that is merely in the page cache cannot gate an effect. Reusing the vocabulary is
//! deliberate: a second durability language would be item 1's whole argument, repeated wrongly.
//!
//! # Failure is a decision, not a log line
//!
//! Three failures, each with a named behaviour (AE0 §5):
//!
//! | Failure | [`EvidenceProfile::Strict`] | [`EvidenceProfile::Lenient`] |
//! |---|---|---|
//! | the queue is full ([`Saturated`](JournalError::Saturated)) | refuse the effect — **never** drop the record silently | log, proceed, and say so in the evidence |
//! | persistence failed (`Failed`) | refuse | log, proceed, and say so |
//! | the acknowledgement is lost ([`DeliveryUnknown`](JournalError::DeliveryUnknown)) | refuse, and the evidence's own delivery is *unknown* — not failed | log, proceed, and say so |
//!
//! The distinction the lenient profile must preserve: proceeding is a choice the operator made, and
//! evidence produced under it is weaker evidence. It says so rather than looking like the other kind.
//!
//! # Five-part statement
//!
//! *Guarantee:* a decision that this journal acknowledges with `OnDisk` is on this node's disk,
//! fsynced, before the dispatch it authorises is allowed to proceed; and nothing written here is
//! gossiped. *Assumptions:* the journal directory is on durable local storage the node owns, and
//! `fsync` means what the filesystem says it means. *Enforcing component:* [`EvidenceJournal::append`]
//! over [`Journal::append`](super::journal::Journal::append) and its writer task. *Failure
//! behaviour:* the three rows above; a full queue is refused rather than dropped, and a lost
//! acknowledgement is reported as unknown rather than as failure. *Detecting tests:* the
//! mechanism's tests in `super::journal` — one per row, plus the round-trip, restart and reader
//! cases — the wrapper pin below, and the no-gossip pin in `action_evaluator`. *Strength:*
//! `SelfImposedPrevention` — it governs this node's own dispatch; it prevents nothing at a resource.

use super::action_evaluator::{AeEvidence, Execution, History, HistoryEntry, HistoryRequest, HistoryScope, HistoryView, RecordKind};
use super::journal::{Journal, JournalCursor};
use std::path::Path;
use std::sync::Arc;

pub use super::journal::{Appended, JournalEntry, JournalError, JournalPage};

/// Where a reader has got to — the journal's cursor, under the name this module always exported.
pub type EvidenceCursor = JournalCursor;

/// The replay-seam stream the evidence journal's queue records under. Fixed, so a trace recorded
/// before the mechanism moved still matches.
const STREAM: &str = "ae/journal";

/// What the operator decided should happen when evidence cannot be established.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceProfile {
    /// Evidence gates the effect: a decision that could not be recorded refuses the dispatch.
    ///
    /// The strong reading of the whole slice — enforcement and attribution stand or fall together.
    Strict,
    /// The dispatch proceeds and the evidence says it was produced under a profile that does not
    /// gate. Weaker, and legible as weaker.
    Lenient,
}

/// The journal handle: the node-local [`Journal`] plus the operator's failure profile.
pub struct EvidenceJournal {
    inner:   Arc<Journal>,
    profile: EvidenceProfile,
}

impl EvidenceJournal {
    /// Open (or create) the evidence journal at `path` and spawn its writer.
    pub fn open(
        path: impl AsRef<Path>,
        profile: EvidenceProfile,
    ) -> Result<Arc<Self>, std::io::Error> {
        Ok(Arc::new(Self { inner: Journal::open(path, STREAM)?, profile }))
    }

    /// A journal whose writer accepts nothing, so every append ends as
    /// [`DeliveryUnknown`](JournalError::DeliveryUnknown).
    ///
    /// Test-only, and it exists because a failing journal is otherwise very hard to arrange from
    /// outside this module — and the behaviour that matters (does the *dispatch* refuse?) lives at
    /// the gateway, not here.
    // Gated on `compliance` as well as `test`: its only caller is the gateway test that reads the
    // audit chain, which needs it. Without that gate it is dead in a test build with no chain.
    #[cfg(all(test, feature = "compliance"))]
    pub(crate) fn stalled(profile: EvidenceProfile) -> Arc<Self> {
        Arc::new(Self { inner: Journal::stalled(STREAM), profile })
    }

    /// The operator's chosen failure behaviour.
    pub fn profile(&self) -> EvidenceProfile {
        self.profile
    }

    /// Where the journal lives.
    pub fn path(&self) -> &Path {
        self.inner.path()
    }

    /// Whether opening it truncated records (a torn tail, or a frame damaged mid-file): a history folded
    /// from it cannot say which records are gone, only that some before the open may be.
    pub(crate) fn truncated_at_open(&self) -> bool {
        self.inner.truncated_at_open()
    }

    /// Append one evidence record, returning only once it is fsynced.
    ///
    /// The content hash is computed over the exact bytes written, so the chain's reference cannot
    /// cite something other than what landed.
    pub async fn append(&self, bytes: Vec<u8>) -> Result<Appended, JournalError> {
        self.inner.append(bytes).await
    }
}

/// Read a bounded page of the evidence journal from `cursor` — the exporter's shape. See
/// [`read_journal_from`](super::journal::read_journal_from).
pub fn read_evidence_journal_from(
    path: &Path,
    cursor: EvidenceCursor,
    max_records: usize,
    max_bytes: usize,
) -> std::io::Result<JournalPage> {
    super::journal::read_journal_from(path, cursor, max_records, max_bytes)
}

// ── The actor history an evaluator may decide on (post-360 row K) ─────────────

/// The actor histories this node keeps from its evidence journal — each actor's recent decisions and their
/// executions, **per enforcement point** — what an evaluator that asks for history decides on.
///
/// **Scope: an enforcement point at this node** ([`HistoryScope::EnforcementPoint`]). The journal is node-local and
/// never gossiped, so a count is of what this point decided. The gateway's doors count together (`gateway`); the
/// provider check counts apart (`provider`): a call the gateway decided and the provider re-checks on one node is
/// one record at each point, never two at one.
///
/// **Loaded in the background, and only when wanted.** The journal is read page by page, off the async workers,
/// once an evaluator that asks for history is attached ([`start_loading`](Self::start_loading)); until the load
/// completes every view is [`History::Unavailable`], so a history-limited rule decides `Indeterminate` rather than
/// count against a history it has not read. Records appended meanwhile are observed live and the load merges
/// under them (by attempt id). A node whose evaluator never asks for history never reads its journal for this.
///
/// **Bounded:** each actor's records within [`HORIZON_MS`] of its newest, at most [`PER_ACTOR`] of them, and at most
/// [`MAX_ACTORS`] actors per point; past it the oldest sixteenth are evicted in one pass, and an actor that appears
/// at the point afterwards — which may have been one of them — has views reaching no further back than what was
/// taken ([`HistoryView::omitted_through_ms`]). Actors that stay are unaffected. Records the journal itself lost — to a
/// repair at open, which a crash's torn tail also triggers, or an append a lenient journal failed to make — make
/// every view reaching back past them incomplete, at every point, for one window.
///
/// **Check, then count, as one step:** an enforcement point holds the actor's [`gate`](Self::gate) from reading the
/// view — with the clock read under it — to counting the permit. A view carries its actor's version too, and a
/// permit is admitted only into the history it was decided on ([`admit`](Self::admit)).
///
/// Two locks (lock-order rows 61 and 62): `gates`, an actor's stripe of 64 async mutexes, held across view,
/// evaluator and admit and released before the journal append; and `state`, a leaf under it, held for a map read or
/// update only (the loader takes it once, to merge).
pub(crate) struct HistoryIndex {
    state: std::sync::Mutex<HistoryState>,
    gates: Vec<tokio::sync::Mutex<()>>,
    journal: std::path::PathBuf,
    /// The journal truncated records when it was opened: what it held before is not all there.
    truncated_at_open: bool,
}

/// How far back an actor's history is kept, from its newest record — 24 hours, and the longest window a fleet-scoped
/// limit may name ([`MAX_FLEET_WINDOW_MS`](super::action_evaluator::MAX_FLEET_WINDOW_MS)): one horizon for both.
pub(crate) const HORIZON_MS: u64 = super::action_evaluator::MAX_FLEET_WINDOW_MS;
/// The most records kept for one actor at one point.
pub(crate) const PER_ACTOR: usize = 1_024;
/// The most actors kept at one point.
pub(crate) const MAX_ACTORS: usize = 65_536;
/// How many stripes actors' decisions are serialised on. Two actors that share one wait for each other.
const GATES: usize = 64;

#[derive(Default)]
enum Load {
    /// Nothing has asked for history yet.
    #[default]
    NotStarted,
    /// The journal is being read.
    Loading,
    /// Read: views are established.
    Loaded,
    /// The journal could not be read: no history here is established.
    Unreadable(String),
}

#[derive(Default)]
struct HistoryState {
    /// (enforcement point, actor) → its history.
    actors: std::collections::HashMap<(String, String), ActorHistory>,
    /// Point → how many actors it holds, against [`MAX_ACTORS`].
    per_point: std::collections::HashMap<String, usize>,
    /// Point → the newest event time of a record an eviction there took. An actor that (re)appears at the point
    /// afterwards may have been among them, so its views reach no further back than this.
    evicted_through_ms: std::collections::HashMap<String, u64>,
    load: Load,
    /// The newest event time of a record the journal itself no longer holds — lost to a repair at open, or an
    /// append a lenient journal failed to make: no view at any point reaching back past it is complete.
    lost_through_ms: Option<u64>,
    /// Executions observed while the journal was still being read, whose decisions the read has not merged yet.
    pending_executions: std::collections::HashMap<String, Execution>,
    /// Records observed, so idle actors are swept every so often rather than on every append.
    observed: u64,
}

#[derive(Default)]
struct ActorHistory {
    /// Oldest first.
    entries: std::collections::VecDeque<HistoryEntry>,
    /// The newest `at_ms` of a record dropped for capacity or age — or, for an actor created after an eviction at
    /// its point, the newest that eviction took, since it may have been this actor's.
    forgotten_through_ms: Option<u64>,
    /// Bumped by every change to what is counted, so a permit is admitted only into the history it was decided on.
    version: u64,
}

fn later(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

impl ActorHistory {
    fn upsert(&mut self, entry: HistoryEntry) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.attempt_id == entry.attempt_id) {
            // The record of an attempt already here (an admitted permit's journal record): the same attempt, so
            // nothing a view counted changes and the version stays — nor its time, which is when it was counted
            // (the record carries the envelope's earlier one; moving it back would let it leave a window early).
            e.decision = entry.decision;
            e.execution = entry.execution;
            return;
        }
        let at = self.entries.iter().rposition(|e| e.at_ms <= entry.at_ms).map_or(0, |i| i + 1);
        self.entries.insert(at, entry);
        self.version += 1;
        let newest = self.entries.back().map_or(0, |e| e.at_ms);
        while let Some(first) = self.entries.front() {
            if self.entries.len() > PER_ACTOR || newest.saturating_sub(first.at_ms) > HORIZON_MS {
                let dropped = self.entries.pop_front().expect("a front");
                self.forgotten_through_ms = later(self.forgotten_through_ms, Some(dropped.at_ms));
            } else {
                break;
            }
        }
    }
}

/// The enforcement point a record's history is kept under: every gateway door is the gateway.
pub(crate) fn point_of(enforcement_point: &str) -> String {
    if enforcement_point.starts_with("gateway") { "gateway".into() } else { enforcement_point.to_string() }
}

fn entry_of(record: &AeEvidence) -> HistoryEntry {
    HistoryEntry {
        at_ms: record.at_ms,
        attempt_id: record.attempt_id.clone(),
        operation: record.operation.clone(),
        resource: record.resource.clone(),
        decision: record.decision,
        execution: record.execution,
    }
}

impl HistoryState {
    /// The actor's history at its point, made room for if it is new: past [`MAX_ACTORS`] at that point, the
    /// sixteenth whose newest records are oldest are evicted in one pass (amortised, never a scan per new actor),
    /// and an actor created after an eviction is marked as reaching no further back than what it took.
    fn actor_mut(&mut self, key: (String, String)) -> &mut ActorHistory {
        if !self.actors.contains_key(&key) {
            let point = key.0.clone();
            let count = self.per_point.entry(point.clone()).or_default();
            if *count >= MAX_ACTORS {
                let mut at_point: Vec<((String, String), u64)> = self
                    .actors
                    .iter()
                    .filter(|(k, _)| k.0 == point)
                    .map(|(k, a)| (k.clone(), a.entries.back().map_or(0, |e| e.at_ms)))
                    .collect();
                at_point.sort_by_key(|(_, t)| *t);
                let mut removed = 0;
                for (victim, newest) in at_point.into_iter().take((MAX_ACTORS / 16).max(1)) {
                    self.actors.remove(&victim);
                    let through = self.evicted_through_ms.entry(point.clone()).or_default();
                    *through = (*through).max(newest);
                    removed += 1;
                }
                let count = self.per_point.entry(point.clone()).or_default();
                *count = count.saturating_sub(removed);
            }
            *self.per_point.entry(point.clone()).or_default() += 1;
            let born = ActorHistory { forgotten_through_ms: self.evicted_through_ms.get(&point).copied(), ..Default::default() };
            self.actors.insert(key.clone(), born);
        }
        self.actors.get_mut(&key).expect("present")
    }

    fn apply(&mut self, record: &AeEvidence) {
        let key = (point_of(&record.enforcement_point), record.subject.clone());
        match record.kind {
            RecordKind::Execution => {
                let loading = matches!(self.load, Load::Loading);
                let found = self.actors.get_mut(&key).and_then(|h| h.entries.iter_mut().find(|e| e.attempt_id == record.attempt_id));
                match found {
                    Some(e) => e.execution = record.execution,
                    // Its decision is still in the journal being read: apply this when the read merges it.
                    None if loading && self.pending_executions.len() < 4_096 => {
                        self.pending_executions.insert(record.attempt_id.clone(), record.execution);
                    }
                    None => {}
                }
            }
            _ => self.actor_mut(key).upsert(entry_of(record)),
        }
        self.observed += 1;
        if self.observed.is_multiple_of(PER_ACTOR as u64) {
            // An actor whose every record has aged out is dropped — nothing of it is inside any window a view
            // can be complete over (a window longer than the horizon never is).
            let newest = self.actors.values().filter_map(|a| a.entries.back().map(|e| e.at_ms)).max().unwrap_or(0);
            self.actors.retain(|_, a| a.entries.back().is_some_and(|e| newest.saturating_sub(e.at_ms) <= HORIZON_MS));
            let mut per_point: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
            for (point, _) in self.actors.keys() {
                *per_point.entry(point.clone()).or_default() += 1;
            }
            self.per_point = per_point;
        }
    }
}

impl HistoryIndex {
    /// An index over `journal`, not yet loaded: [`start_loading`](Self::start_loading) reads it.
    pub(crate) fn new(journal: &EvidenceJournal) -> Self {
        HistoryIndex {
            state: std::sync::Mutex::new(HistoryState::default()),
            gates: (0..GATES).map(|_| tokio::sync::Mutex::new(())).collect(),
            journal: journal.path().to_path_buf(),
            truncated_at_open: journal.truncated_at_open(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HistoryState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Read the journal into the index, once, on a thread of its own. `now_ms` dates a journal repair: records
    /// before it may be missing.
    pub(crate) fn start_loading(self: &std::sync::Arc<Self>, now_ms: u64) {
        {
            let mut state = self.lock();
            if !matches!(state.load, Load::NotStarted) {
                return;
            }
            state.load = Load::Loading;
        }
        let index = std::sync::Arc::clone(self);
        std::thread::spawn(move || index.load(now_ms));
    }

    /// Page through the journal into a map of its own, then merge it under what was observed meanwhile.
    pub(crate) fn load(&self, now_ms: u64) {
        let mut loaded = HistoryState::default();
        let mut cursor = EvidenceCursor::default();
        let outcome: Result<(), String> = loop {
            match read_evidence_journal_from(&self.journal, cursor, 1_024, 8 * 1024 * 1024) {
                Ok(page) => {
                    let mut unreadable = None;
                    for entry in &page.entries {
                        match serde_json::from_slice::<AeEvidence>(&entry.bytes) {
                            Ok(record) => loaded.apply(&record),
                            Err(e) => {
                                unreadable = Some(format!("an evidence record could not be read: {e}"));
                                break;
                            }
                        }
                    }
                    if let Some(why) = unreadable {
                        break Err(why);
                    }
                    cursor = page.next;
                    if !page.more {
                        break Ok(());
                    }
                }
                // A journal not yet written is an empty history, not an unreadable one.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break Ok(()),
                Err(e) => break Err(format!("the evidence journal could not be read: {e}")),
            }
        };
        let mut state = self.lock();
        match outcome {
            Err(why) => state.load = Load::Unreadable(why),
            Ok(()) => {
                let pending = std::mem::take(&mut state.pending_executions);
                for (key, history) in loaded.actors {
                    let forgotten = history.forgotten_through_ms;
                    let live = state.actor_mut(key);
                    for mut e in history.entries {
                        if let Some(x) = pending.get(&e.attempt_id) {
                            e.execution = *x;
                        }
                        // What was observed live is the newer word on an attempt: a loaded copy fills gaps only.
                        if !live.entries.iter().any(|l| l.attempt_id == e.attempt_id) {
                            live.upsert(e);
                        }
                    }
                    live.forgotten_through_ms = later(live.forgotten_through_ms, forgotten);
                }
                for (point, t) in loaded.evicted_through_ms {
                    let through = state.evicted_through_ms.entry(point).or_default();
                    *through = (*through).max(t);
                }
                state.lost_through_ms = later(state.lost_through_ms, loaded.lost_through_ms);
                if self.truncated_at_open {
                    // The journal dropped records when it was opened, and which is not known: none before now
                    // can be vouched for.
                    state.lost_through_ms = later(state.lost_through_ms, Some(now_ms));
                }
                state.load = Load::Loaded;
            }
        }
    }

    /// The actor's stripe: hold it from reading the view to admitting the permit.
    pub(crate) fn gate(&self, actor: &str) -> &tokio::sync::Mutex<()> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        actor.hash(&mut h);
        &self.gates[(h.finish() as usize) % GATES]
    }

    /// The actor's history at `point` for `request` as of `now_ms`, and the version a permit decided on it is
    /// admitted at.
    pub(crate) fn view(&self, point: &str, actor: &str, request: HistoryRequest, now_ms: u64) -> (History, u64) {
        let state = self.lock();
        match &state.load {
            Load::Loaded => {}
            Load::Unreadable(why) => return (History::Unavailable(why.clone()), 0),
            Load::NotStarted | Load::Loading => {
                return (History::Unavailable("the history is still being read from the evidence journal".into()), 0)
            }
        }
        let from = now_ms.saturating_sub(request.window_ms);
        let key = (point.to_string(), actor.to_string());
        let (in_window, forgotten, version): (Vec<&HistoryEntry>, Option<u64>, u64) = match state.actors.get(&key) {
            // Everything after the window's start — a record stamped after `now_ms` is one more, not none.
            Some(h) => (h.entries.iter().filter(|e| e.at_ms > from).collect(), h.forgotten_through_ms, h.version),
            // Not held: it may have been evicted from this point, so what was evicted bounds its view too.
            None => (Vec::new(), state.evicted_through_ms.get(point).copied(), 0),
        };
        let take = in_window.len().min(request.max_entries);
        let cut = in_window.len() - take;
        let entries: Vec<HistoryEntry> = in_window[cut..].iter().map(|e| (*e).clone()).collect();
        let mut omitted = later(forgotten, state.lost_through_ms);
        if cut > 0 {
            omitted = later(omitted, Some(in_window[cut - 1].at_ms));
        }
        if request.window_ms > HORIZON_MS {
            // Older than the horizon, nothing is kept: the window reaches past what is held.
            omitted = later(omitted, Some(now_ms.saturating_sub(HORIZON_MS)));
        }
        let view = HistoryView {
            scope: HistoryScope::EnforcementPoint,
            actor: actor.to_string(),
            point: point.to_string(),
            as_of_ms: now_ms,
            window_ms: request.window_ms,
            entries,
            complete: false,
            omitted_through_ms: omitted,
            fleet: Vec::new(),
        };
        let complete = view.complete_for(request.window_ms);
        (History::View(HistoryView { complete, ..view }), version)
    }

    /// Count a permit decided on the view at `version`: `true` if the actor's history at `point` is still that one,
    /// and the permit is now in it; `false` if it changed meanwhile — decide again on a fresh view.
    pub(crate) fn admit(&self, point: &str, actor: &str, version: u64, entry: HistoryEntry) -> bool {
        let mut state = self.lock();
        let h = state.actor_mut((point.to_string(), actor.to_string()));
        if h.version != version {
            return false;
        }
        h.upsert(entry);
        true
    }

    /// Take back a permit admitted for a decision that was then not recorded — and so did not happen.
    pub(crate) fn retract(&self, point: &str, actor: &str, attempt_id: &str) {
        let mut state = self.lock();
        if let Some(h) = state.actors.get_mut(&(point.to_string(), actor.to_string())) {
            h.entries.retain(|e| e.attempt_id != attempt_id);
            h.version += 1;
        }
    }

    /// A record this point appended: a decision enters its actor's history, an execution updates it.
    pub(crate) fn observe(&self, record: &AeEvidence) {
        self.lock().apply(record);
    }

    /// A record that was decided but not appended (a lenient journal proceeding past a failed append): after a
    /// restart the journal will not hold it, so no view reaching back past it can be complete.
    pub(crate) fn note_lost(&self, at_ms: u64) {
        let mut state = self.lock();
        state.lost_through_ms = later(state.lost_through_ms, Some(at_ms));
    }
}

/// Read every evidence record back, in order. **The exporter should prefer
/// [`read_evidence_journal_from`]** — this one is for tests and small one-shot inspections.
pub fn read_evidence_journal(path: &Path) -> std::io::Result<Vec<Vec<u8>>> {
    super::journal::read_journal(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycelium_core::receipt::LocalDurability;

    /// **The wrapper pin.** The public surface this module always had — open with a profile,
    /// append for an `OnDisk` receipt, read back — still holds, and the queue records under the
    /// stream it always did, so a replay trace from before the split still matches.
    #[tokio::test]
    async fn the_wrapper_keeps_its_surface_and_its_stream() {
        let dir = std::env::temp_dir().join(format!("ae-wrapper-{}", fastrand::u64(..)));
        let path = dir.join("evidence.log");
        let j = EvidenceJournal::open(&path, EvidenceProfile::Strict).unwrap();

        let a = j.append(b"decision".to_vec()).await.expect("appended");
        assert_eq!(a.durability, LocalDurability::OnDisk);
        assert_eq!(j.profile(), EvidenceProfile::Strict);
        assert_eq!(j.path(), path.as_path());
        assert_eq!(j.inner.stream(), "ae/journal", "the trace stream must not move with the code");
        assert_eq!(read_evidence_journal(&path).unwrap(), vec![b"decision".to_vec()]);

        let page = read_evidence_journal_from(&path, EvidenceCursor::default(), 10, usize::MAX).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].content_hash, a.content_hash);
    }
}


#[cfg(test)]
mod history_tests {
    //! The actor histories an evaluator may decide on (post-360 row K).
    use super::*;
    use crate::agent::action_evaluator::{ActionEnvelope, ActionMapping, Decision, DecisionKind, Execution, Verdict};
    use mycelium_core::NodeId;

    const WINDOW: u64 = 600_000;
    const GW: &str = "gateway";

    fn record_at(point: &str, actor: &str, attempt: &str, at_ms: u64, verdict: Verdict) -> AeEvidence {
        let env = ActionEnvelope::builder(actor, NodeId::new("127.0.0.1", 9000).unwrap(), "tools/call", "tool:post@n1")
            .identities(format!("op-{attempt}"), attempt)
            .mapping(ActionMapping::mapped("cat", "1"))
            .validity(at_ms, at_ms + 60_000)
            .build();
        let d = Decision { verdict, checked: vec![], reason: String::new(), policy_revision: "rev".into(), errors: vec![] };
        let execution = if verdict == Verdict::Permit { Execution::Attempted } else { Execution::None };
        AeEvidence::for_decision(&env, &d, execution, point)
    }

    fn record(actor: &str, attempt: &str, at_ms: u64, verdict: Verdict) -> AeEvidence {
        record_at("gateway:mcp/tools/call", actor, attempt, at_ms, verdict)
    }

    fn request() -> HistoryRequest {
        HistoryRequest { window_ms: WINDOW, max_entries: 10 }
    }

    fn view(h: History) -> HistoryView {
        match h {
            History::View(v) => v,
            History::Unavailable(why) => panic!("unavailable: {why}"),
        }
    }

    fn temp_journal(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("history-{tag}-{}", fastrand::u64(..))).join("evidence.log")
    }

    /// A loaded index over an empty journal, and the journal (held so its lock lasts the test).
    fn loaded(tag: &str) -> (Arc<EvidenceJournal>, HistoryIndex) {
        let j = EvidenceJournal::open(temp_journal(tag), EvidenceProfile::Strict).unwrap();
        let index = HistoryIndex::new(&j);
        index.load(0);
        (j, index)
    }

    /// The history survives a restart: it is read from what the journal holds.
    #[tokio::test]
    async fn a_history_is_loaded_from_the_journal() {
        let path = temp_journal("load");
        let j = EvidenceJournal::open(&path, EvidenceProfile::Strict).unwrap();
        for (i, at) in [1_000u64, 2_000, 3_000].iter().enumerate() {
            j.append(serde_json::to_vec(&record("alice", &format!("a{i}"), *at, Verdict::Permit)).unwrap()).await.unwrap();
        }
        j.append(serde_json::to_vec(&record("bob", "b0", 2_500, Verdict::Permit)).unwrap()).await.unwrap();
        let index = HistoryIndex::new(&j);
        index.load(10_000);
        let v = view(index.view(GW, "alice", request(), 10_000).0);
        assert_eq!(v.entries.iter().map(|e| e.attempt_id.as_str()).collect::<Vec<_>>(), ["a0", "a1", "a2"]);
        assert!(v.complete && v.scope == HistoryScope::EnforcementPoint && v.point == GW && v.as_of_ms == 10_000);
        assert_eq!(view(index.view(GW, "bob", request(), 10_000).0).entries.len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Until the journal is read, no history is established — never an empty one.
    #[tokio::test]
    async fn a_history_not_yet_read_is_unavailable() {
        let j = EvidenceJournal::open(temp_journal("unread"), EvidenceProfile::Strict).unwrap();
        let (h, _) = HistoryIndex::new(&j).view(GW, "alice", request(), 10_000);
        assert!(matches!(h, History::Unavailable(ref why) if why.contains("still being read")), "{h:?}");
    }

    #[tokio::test]
    async fn a_journal_not_yet_written_is_an_empty_history() {
        let (_j, index) = loaded("absent");
        let v = view(index.view(GW, "alice", request(), 10_000).0);
        assert!(v.entries.is_empty() && v.complete);
    }

    /// A journal that cannot be read gives no history, never an empty one.
    #[tokio::test]
    async fn an_unreadable_journal_makes_every_history_unavailable() {
        let path = temp_journal("unreadable");
        let j = EvidenceJournal::open(&path, EvidenceProfile::Strict).unwrap();
        j.append(b"not an evidence record".to_vec()).await.unwrap();
        let index = HistoryIndex::new(&j);
        index.load(10_000);
        let (h, _) = index.view(GW, "alice", request(), 10_000);
        assert!(matches!(h, History::Unavailable(ref why) if why.contains("could not be read")), "{h:?}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A journal that dropped records when it was opened vouches for nothing before then (review finding 5).
    #[tokio::test]
    async fn a_journal_repaired_at_open_makes_older_history_incomplete() {
        let path = temp_journal("repaired");
        {
            let j = EvidenceJournal::open(&path, EvidenceProfile::Strict).unwrap();
            for i in 0..3u64 {
                j.append(serde_json::to_vec(&record("alice", &format!("a{i}"), 9_000 + i, Verdict::Permit)).unwrap()).await.unwrap();
            }
        }
        // Damage the second record's length prefix: the reopen truncates there, dropping two records.
        let mut bytes = std::fs::read(&path).unwrap();
        let first = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        bytes[4 + first..4 + first + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let j = EvidenceJournal::open(&path, EvidenceProfile::Strict).unwrap();
        assert!(j.truncated_at_open());
        let index = HistoryIndex::new(&j);
        index.load(10_000);
        let v = view(index.view(GW, "alice", request(), 10_000).0);
        assert_eq!(v.entries.len(), 1, "two records are gone");
        assert!(!v.complete, "and the view says the history before the open cannot be vouched for");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The gateway's doors count together; the provider check counts apart (review finding 1).
    #[tokio::test]
    async fn each_enforcement_point_counts_its_own_decisions() {
        let (_j, index) = loaded("points");
        index.observe(&record_at("gateway:mcp/tools/call", "alice", "g1", 9_000, Verdict::Permit));
        index.observe(&record_at("gateway:a2a", "alice", "g2", 9_100, Verdict::Permit));
        index.observe(&record_at("provider", "alice", "p1", 9_000, Verdict::Permit));
        assert_eq!(view(index.view(GW, "alice", request(), 10_000).0).entries.len(), 2);
        assert_eq!(view(index.view("provider", "alice", request(), 10_000).0).entries.len(), 1);
    }

    /// A record stamped after the view's clock — a call that read its clock later, and was counted first — is
    /// counted, not left out (review finding 2).
    #[tokio::test]
    async fn a_record_stamped_after_the_views_clock_is_counted() {
        let (_j, index) = loaded("later");
        index.observe(&record("alice", "later", 10_050, Verdict::Permit));
        assert_eq!(view(index.view(GW, "alice", request(), 10_000).0).entries.len(), 1);
    }

    /// A permit decided on a view is counted only into the history it was decided on.
    #[tokio::test]
    async fn a_permit_is_admitted_only_at_the_version_it_was_decided_on() {
        let (_j, index) = loaded("admit");
        let (_, v0) = index.view(GW, "alice", request(), 10_000);
        let (_, also_v0) = index.view(GW, "alice", request(), 10_000);
        let entry = |a: &str| HistoryEntry {
            at_ms: 9_000, attempt_id: a.into(), operation: "tools/call".into(), resource: "tool:post@n1".into(),
            decision: DecisionKind::Permit, execution: Execution::Attempted,
        };
        assert!(index.admit(GW, "alice", v0, entry("first")));
        assert!(!index.admit(GW, "alice", also_v0, entry("second")), "decided on a history that has since changed");
        let (h, v1) = index.view(GW, "alice", request(), 10_000);
        assert_eq!(view(h).entries.len(), 1);
        assert!(index.admit(GW, "alice", v1, entry("second")));
        index.retract(GW, "alice", "second");
        assert_eq!(view(index.view(GW, "alice", request(), 10_000).0).entries.len(), 1);
    }

    /// The record that follows an admitted permit replaces it, and an execution updates it — no double count.
    #[tokio::test]
    async fn the_journal_record_of_an_admitted_permit_is_not_counted_twice() {
        let (_j, index) = loaded("observe");
        let (_, v) = index.view(GW, "alice", request(), 10_000);
        let r = record("alice", "p1", 9_000, Verdict::Permit);
        assert!(index.admit(GW, "alice", v, HistoryEntry {
            at_ms: r.at_ms, attempt_id: r.attempt_id.clone(), operation: r.operation.clone(), resource: r.resource.clone(),
            decision: DecisionKind::Permit, execution: Execution::Attempted,
        }));
        index.observe(&r);
        index.observe(&r.as_execution(Execution::Completed));
        let v = view(index.view(GW, "alice", request(), 10_000).0);
        assert_eq!(v.entries.len(), 1);
        assert_eq!(v.entries[0].execution, Execution::Completed);
    }

    /// A dropped record makes incomplete only the windows that reach back past it (review finding 3).
    #[tokio::test]
    async fn completeness_is_judged_per_window() {
        let (_j, index) = loaded("cap");
        for i in 0..(PER_ACTOR as u64 + 5) {
            index.observe(&record("alice", &format!("p{i}"), 100_000 + i, Verdict::Permit));
        }
        let wide = view(index.view(GW, "alice", HistoryRequest { window_ms: WINDOW, max_entries: usize::MAX }, 200_000).0);
        assert_eq!(wide.entries.len(), PER_ACTOR);
        assert!(!wide.complete, "the five oldest were dropped and are inside the window");
        assert!(wide.complete_for(99_000), "a window that starts after them sees everything it needs");
        let few = view(index.view(GW, "alice", HistoryRequest { window_ms: WINDOW, max_entries: 3 }, 200_000).0);
        assert_eq!(few.entries.len(), 3);
        assert!(!few.complete);
    }

    #[tokio::test]
    async fn a_window_longer_than_the_horizon_is_never_complete() {
        let (_j, index) = loaded("horizon");
        let now = 10 * HORIZON_MS; // a real clock, long past the horizon
        index.observe(&record("alice", "p", now - 1_000, Verdict::Permit));
        let long = HistoryRequest { window_ms: HORIZON_MS + 1, max_entries: 10 };
        assert!(!view(index.view(GW, "alice", long, now).0).complete);
        assert!(!view(index.view(GW, "bob", long, now).0).complete);
        // And it does not spoil a short window read from the same view.
        assert!(view(index.view(GW, "alice", long, now).0).complete_for(600_000));
    }

    /// A lenient journal that proceeded past a failed append leaves a hole a restart will not refill.
    #[tokio::test]
    async fn a_lost_record_makes_windows_over_it_incomplete() {
        let (_j, index) = loaded("lost");
        index.note_lost(9_500);
        assert!(!view(index.view(GW, "alice", request(), 10_000).0).complete);
        assert!(view(index.view(GW, "alice", HistoryRequest { window_ms: 400, max_entries: 10 }, 10_000).0).complete);
    }

    /// Bounded in actors too: one evicted for room takes its records, and views reaching back past them say so.
    #[tokio::test]
    async fn an_actor_evicted_for_room_leaves_older_views_incomplete() {
        let (_j, index) = loaded("actors");
        for i in 0..MAX_ACTORS as u64 {
            index.observe(&record(&format!("actor-{i}"), "a", 1_000 + i, Verdict::Permit));
        }
        index.observe(&record("newcomer", "n", 900_000, Verdict::Permit));
        assert!(!view(index.view(GW, "newcomer", HistoryRequest { window_ms: 900_000, max_entries: 10 }, 900_001).0).complete);
        assert!(view(index.view(GW, "newcomer", HistoryRequest { window_ms: 1_000, max_entries: 10 }, 900_001).0).complete);
        // An actor never seen here may have been evicted too.
        assert!(!view(index.view(GW, "stranger", HistoryRequest { window_ms: 900_000, max_entries: 10 }, 900_001).0).complete);
        // One that stayed is untouched by the others' eviction, and another point not at all (review N1).
        let stayed = format!("actor-{}", MAX_ACTORS - 1);
        assert!(view(index.view(GW, &stayed, HistoryRequest { window_ms: 900_000, max_entries: 10 }, 900_001).0).complete);
        assert!(view(index.view("provider", "alice", HistoryRequest { window_ms: 900_000, max_entries: 10 }, 900_001).0).complete);
        // Bounded per point: the eviction made room in one pass.
        assert!(index.lock().per_point[GW] <= MAX_ACTORS);
    }

    /// The journal record of an admitted permit keeps the time it was counted at (review N2).
    #[tokio::test]
    async fn an_admitted_permit_keeps_the_time_it_was_counted_at() {
        let (_j, index) = loaded("time");
        let (_, v) = index.view(GW, "alice", request(), 10_000);
        let r = record("alice", "p1", 1_000, Verdict::Permit); // its envelope was assembled at 1 000
        assert!(index.admit(GW, "alice", v, HistoryEntry {
            at_ms: 5_000, attempt_id: r.attempt_id.clone(), operation: r.operation.clone(), resource: r.resource.clone(),
            decision: DecisionKind::Permit, execution: Execution::Attempted,
        }));
        index.observe(&r);
        assert_eq!(view(index.view(GW, "alice", request(), 10_000).0).entries[0].at_ms, 5_000);
    }

    /// An execution observed while the journal is still being read is not lost when the read merges (review N4).
    #[tokio::test]
    async fn an_execution_seen_during_the_read_survives_the_merge() {
        let path = temp_journal("pending");
        let j = EvidenceJournal::open(&path, EvidenceProfile::Strict).unwrap();
        let r = record("alice", "p1", 9_000, Verdict::Permit);
        j.append(serde_json::to_vec(&r).unwrap()).await.unwrap();
        let index = HistoryIndex::new(&j);
        index.lock().load = Load::Loading;
        index.observe(&r.as_execution(Execution::Completed));
        index.load(10_000);
        assert_eq!(view(index.view(GW, "alice", request(), 10_000).0).entries[0].execution, Execution::Completed);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}

#[cfg(test)]
mod crash_state_tests {
    use super::*;
    use crate::agent::action_evaluator::{
        ActionEnvelope, ActionMapping, AeEvidence, Decision, Execution,
    };
    use crate::node_id::NodeId;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "mycelium-ae3-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("temp dir");
        d
    }

    fn evidence(execution: Execution) -> AeEvidence {
        let via = NodeId::new("127.0.0.1", 9000).expect("node id");
        let env = ActionEnvelope::builder("oidc:idp/dispatcher", via, "tools/call", "tool:pay@n1")
            .identities("op-9", "op-9/1")
            .mapping(ActionMapping::mapped("cat", "1"))
            .validity(1_000, 61_000)
            .build();
        AeEvidence::for_decision(
            &env,
            &Decision::permit("within remit", "rev-1"),
            execution,
            "depot-resource",
        )
    }

    /// **AE3: after a crash before the execution record, the journal holds the decision and
    /// invents nothing.**
    ///
    /// The effect happened; the record saying so never reached the journal. On restart the evidence
    /// holds exactly what was acknowledged and no more — in particular **no attested refusal**,
    /// because `Execution::None` is a claim this node never made and a journal that manufactured
    /// one would hand a reader an all-clear out of a crash.
    ///
    /// Driven through the **real** journal, reopened from disk, because the AE3 gate asks for these
    /// states from a journal rather than from records a test assembled.
    ///
    /// **What this does not prove, and it is the larger half.** Whether a *reader* then refuses to
    /// read the absent record as "nothing happened" is a property of the reader, not of the
    /// journal — AE0 §5's rule that *silence is never reassurance* is enforced in the correlator,
    /// where it has its own test. This one establishes only that the journal gives that reader an
    /// honest input: what was acknowledged, nothing more, nothing invented.
    #[tokio::test]
    async fn a_crash_before_the_execution_record_leaves_only_what_was_acknowledged() {
        let dir = tmpdir("before");
        let path = dir.join("evidence.log");

        {
            let journal = EvidenceJournal::open(&path, EvidenceProfile::Strict).expect("open");
            let decided = evidence(Execution::Attempted);
            let bytes = serde_json::to_vec(&decided).expect("encode");
            journal.append(bytes).await.expect("the decision is recorded");
            // ... the effect runs, and the process dies here. No execution record is appended.
        }

        // Restart: a fresh reader over the same file.
        let back: Vec<AeEvidence> = read_evidence_journal(&path)
            .expect("the journal survives a crash")
            .into_iter()
            .map(|b| serde_json::from_slice(&b).expect("a record decodes"))
            .collect();

        assert_eq!(back.len(), 1, "exactly what was acknowledged is what survives");
        assert_eq!(back[0].operation_id, "op-9");
        assert_eq!(
            back[0].execution,
            Execution::Attempted,
            "what it recorded is what it recorded: dispatched, outcome unobserved",
        );
        assert!(
            !back.iter().any(|r| r.execution == Execution::None),
            "no surviving record may attest a refusal — `None` is a claim this node never made, \
             and a journal that produced one out of a crash would hand a reader an all-clear",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **AE3: a crash *after* the append, before the acknowledgement reaches the caller.**
    ///
    /// The caller never learned the record landed — and it did. The evidence must hold it, because
    /// the alternative is a caller that retries against a world where the effect is already
    /// recorded, and evidence that disagrees with itself across a restart.
    ///
    /// This is the direction people expect to be safe and is worth pinning anyway: a lost
    /// *acknowledgement* is not a lost *record*, and item 1 draws that distinction everywhere else.
    #[tokio::test]
    async fn a_crash_after_the_append_still_holds_the_record() {
        let dir = tmpdir("after");
        let path = dir.join("evidence.log");

        {
            let journal = EvidenceJournal::open(&path, EvidenceProfile::Strict).expect("open");
            journal
                .append(serde_json::to_vec(&evidence(Execution::Attempted)).expect("encode"))
                .await
                .expect("decision recorded");
            // The append returns only once fsynced, so this record is on disk before the
            // acknowledgement below is even constructed.
            journal
                .append(serde_json::to_vec(&evidence(Execution::Completed)).expect("encode"))
                .await
                .expect("execution recorded");
            // ... the acknowledgement is lost on the way back to the caller, and the process dies.
        }

        let back: Vec<AeEvidence> = read_evidence_journal(&path)
            .expect("read")
            .into_iter()
            .map(|b| serde_json::from_slice(&b).expect("decodes"))
            .collect();

        assert_eq!(back.len(), 2, "a lost acknowledgement is not a lost record");
        assert_eq!(back[1].execution, Execution::Completed);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The two crash points are **distinguishable** from the evidence alone, which is the property
    /// that makes them useful rather than merely honest.
    ///
    /// If a reader could not tell "we never recorded the outcome" from "we recorded that it
    /// completed", every crash would collapse into one indistinguishable shrug — and an operator
    /// asking *did it happen?* would get the same non-answer either way.
    #[tokio::test]
    async fn the_two_crash_points_are_distinguishable_from_the_evidence() {
        let before = tmpdir("dist-before");
        let after = tmpdir("dist-after");
        let (pb, pa) = (before.join("e.log"), after.join("e.log"));

        {
            let j = EvidenceJournal::open(&pb, EvidenceProfile::Strict).expect("open");
            j.append(serde_json::to_vec(&evidence(Execution::Attempted)).expect("enc"))
                .await
                .expect("append");
        }
        {
            let j = EvidenceJournal::open(&pa, EvidenceProfile::Strict).expect("open");
            j.append(serde_json::to_vec(&evidence(Execution::Attempted)).expect("enc"))
                .await
                .expect("append");
            j.append(serde_json::to_vec(&evidence(Execution::Completed)).expect("enc"))
                .await
                .expect("append");
        }

        let count = |p: &std::path::Path| read_evidence_journal(p).expect("read").len();
        assert_ne!(
            count(&pb),
            count(&pa),
            "a crash before the outcome and a crash after it must not read the same",
        );

        let _ = std::fs::remove_dir_all(&before);
        let _ = std::fs::remove_dir_all(&after);
    }
}
