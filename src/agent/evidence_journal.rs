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

use super::action_evaluator::{AeEvidence, History, HistoryEntry, HistoryRequest, HistoryScope, HistoryView, RecordKind};
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

/// The actor histories this enforcement point keeps, from its evidence journal: each actor's recent
/// decisions and their executions, what an evaluator that asks for history decides on.
///
/// **Scope: this enforcement point** ([`HistoryScope::EnforcementPoint`]) — the journal is node-local and never
/// gossiped, so a count is of what this point decided. **Loaded** from the journal when it is attached (a
/// restart keeps the history the journal holds) and **fed** by every record this point appends after. A
/// journal that cannot be read makes every history [`History::Unavailable`], never empty.
///
/// **Bounded:** an actor's records within [`HORIZON_MS`] of the newest, at most [`PER_ACTOR`] of them; an
/// actor with nothing left in the horizon is forgotten. A view reaching past what was dropped says it is
/// incomplete, and a window longer than the horizon is always incomplete.
///
/// **Check, then count, as one step:** an enforcement point holds the actor's [`gate`](Self::gate) from
/// reading the view to counting the permit, so concurrent calls by one actor are decided one after another
/// and cannot both slip under the same three-in-ten-minutes. Belt and braces, a view also carries its actor's
/// version, and a permit is admitted only into the history it was decided on ([`admit`](Self::admit)).
///
/// Two locks (lock-order rows 61 and 62): `gates`, an actor's stripe of 64 async mutexes, held across view,
/// evaluator and admit and released before the journal append; and `state`, a leaf under it, held for a map
/// read or update only.
pub(crate) struct HistoryIndex {
    state: std::sync::Mutex<HistoryState>,
    gates: Vec<tokio::sync::Mutex<()>>,
}

/// How many stripes actors' decisions are serialised on. Two actors that share one wait for each other.
const GATES: usize = 64;

/// How far back an actor's history is kept, from its newest record.
pub(crate) const HORIZON_MS: u64 = 24 * 60 * 60 * 1_000;
/// The most records kept for one actor.
pub(crate) const PER_ACTOR: usize = 1_024;

#[derive(Default)]
struct HistoryState {
    actors: std::collections::HashMap<String, ActorHistory>,
    /// The journal could not be read at attach: no history here is established.
    unreadable: Option<String>,
    /// Records observed, so idle actors are swept every so often rather than on every append.
    observed: u64,
}

#[derive(Default)]
struct ActorHistory {
    /// Oldest first.
    entries: std::collections::VecDeque<HistoryEntry>,
    /// The newest `at_ms` of a record dropped for capacity or age: a window reaching back past it is incomplete.
    forgotten_through_ms: Option<u64>,
    /// Bumped by every change, so a permit decided on a view is admitted only into the history it was decided on.
    version: u64,
}

impl ActorHistory {
    fn upsert(&mut self, entry: HistoryEntry) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.attempt_id == entry.attempt_id) {
            // The record of an attempt already here (an admitted permit's journal record, its execution): the
            // same attempt, so nothing a view counted changes and the version stays.
            *e = entry;
            return;
        }
        let at = self.entries.iter().rposition(|e| e.at_ms <= entry.at_ms).map_or(0, |i| i + 1);
        self.entries.insert(at, entry);
        self.version += 1;
        self.prune();
    }

    fn prune(&mut self) {
        let newest = self.entries.back().map_or(0, |e| e.at_ms);
        while let Some(first) = self.entries.front() {
            if self.entries.len() > PER_ACTOR || newest.saturating_sub(first.at_ms) > HORIZON_MS {
                let dropped = self.entries.pop_front().expect("a front");
                self.forgotten_through_ms = Some(self.forgotten_through_ms.unwrap_or(0).max(dropped.at_ms));
            } else {
                break;
            }
        }
    }
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

impl HistoryIndex {
    /// An index loaded from the journal at `path`; a journal that cannot be read is recorded as such.
    pub(crate) fn load(path: &Path) -> Self {
        let index = HistoryIndex {
            state: std::sync::Mutex::new(HistoryState::default()),
            gates: (0..GATES).map(|_| tokio::sync::Mutex::new(())).collect(),
        };
        match read_evidence_journal(path) {
            Ok(records) => {
                for bytes in records {
                    match serde_json::from_slice::<AeEvidence>(&bytes) {
                        Ok(record) => index.observe(&record),
                        Err(e) => {
                            index.lock().unreadable = Some(format!("an evidence record could not be read: {e}"));
                            break;
                        }
                    }
                }
            }
            // A journal not yet written is an empty history, not an unreadable one.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => index.lock().unreadable = Some(format!("the evidence journal could not be read: {e}")),
        }
        index
    }

    /// The actor's stripe: hold it from reading the view to admitting the permit.
    pub(crate) fn gate(&self, actor: &str) -> &tokio::sync::Mutex<()> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        actor.hash(&mut h);
        &self.gates[(h.finish() as usize) % GATES]
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HistoryState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The actor's history for `request` as of `now_ms`, and the version a permit decided on it is admitted at.
    pub(crate) fn view(&self, actor: &str, request: HistoryRequest, now_ms: u64) -> (History, u64) {
        let state = self.lock();
        if let Some(why) = &state.unreadable {
            return (History::Unavailable(why.clone()), 0);
        }
        let from = now_ms.saturating_sub(request.window_ms);
        let Some(h) = state.actors.get(actor) else {
            let view = HistoryView {
                scope: HistoryScope::EnforcementPoint,
                actor: actor.to_string(),
                as_of_ms: now_ms,
                window_ms: request.window_ms,
                entries: Vec::new(),
                complete: request.window_ms <= HORIZON_MS,
            };
            return (History::View(view), 0);
        };
        let in_window: Vec<&HistoryEntry> = h.entries.iter().filter(|e| e.at_ms > from && e.at_ms <= now_ms).collect();
        let take = in_window.len().min(request.max_entries);
        let entries: Vec<HistoryEntry> = in_window[in_window.len() - take..].iter().map(|e| (*e).clone()).collect();
        let complete = take == in_window.len()
            && request.window_ms <= HORIZON_MS
            && h.forgotten_through_ms.is_none_or(|t| t <= from);
        let view = HistoryView {
            scope: HistoryScope::EnforcementPoint,
            actor: actor.to_string(),
            as_of_ms: now_ms,
            window_ms: request.window_ms,
            entries,
            complete,
        };
        (History::View(view), h.version)
    }

    /// Count a permit decided on the view at `version`: `true` if the actor's history is still that one, and
    /// the permit is now in it; `false` if it changed meanwhile — decide again on a fresh view.
    pub(crate) fn admit(&self, actor: &str, version: u64, entry: HistoryEntry) -> bool {
        let mut state = self.lock();
        let h = state.actors.entry(actor.to_string()).or_default();
        if h.version != version {
            return false;
        }
        h.upsert(entry);
        true
    }

    /// Take back a permit admitted for a decision that was then not recorded — and so did not happen.
    pub(crate) fn retract(&self, actor: &str, attempt_id: &str) {
        let mut state = self.lock();
        if let Some(h) = state.actors.get_mut(actor) {
            h.entries.retain(|e| e.attempt_id != attempt_id);
            h.version += 1;
        }
    }

    /// A record this point appended: a decision enters its actor's history, an execution updates it.
    pub(crate) fn observe(&self, record: &AeEvidence) {
        let mut state = self.lock();
        let h = state.actors.entry(record.subject.clone()).or_default();
        match record.kind {
            RecordKind::Execution => {
                if let Some(e) = h.entries.iter_mut().find(|e| e.attempt_id == record.attempt_id) {
                    e.execution = record.execution;
                }
            }
            _ => h.upsert(entry_of(record)),
        }
        // Every so often, an actor whose every record has aged out is forgotten — nothing of it is inside any
        // window a view can be complete over.
        state.observed += 1;
        if state.observed.is_multiple_of(PER_ACTOR as u64) {
            let newest = state.actors.values().filter_map(|a| a.entries.back().map(|e| e.at_ms)).max().unwrap_or(0);
            state.actors.retain(|_, a| a.entries.back().is_some_and(|e| newest.saturating_sub(e.at_ms) <= HORIZON_MS));
        }
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

    const WINDOW: u64 = 600_000;

    fn record(actor: &str, attempt: &str, at_ms: u64, verdict: Verdict) -> AeEvidence {
        let env = ActionEnvelope::builder(actor, NodeId::new("127.0.0.1", 9000).unwrap(), "tools/call", "tool:post@n1")
            .identities(format!("op-{attempt}"), attempt)
            .mapping(ActionMapping::mapped("cat", "1"))
            .validity(at_ms, at_ms + 60_000)
            .build();
        let d = Decision { verdict, checked: vec![], reason: String::new(), policy_revision: "rev".into(), errors: vec![] };
        AeEvidence::for_decision(&env, &d, if verdict == Verdict::Permit { Execution::Attempted } else { Execution::None }, "gateway")
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
        let dir = std::env::temp_dir().join(format!("history-{tag}-{}", fastrand::u64(..)));
        dir.join("evidence.log")
    }

    use mycelium_core::NodeId;

    /// The history survives a restart: it is loaded from what the journal holds.
    #[tokio::test]
    async fn a_history_is_loaded_from_the_journal() {
        let path = temp_journal("load");
        let j = EvidenceJournal::open(&path, EvidenceProfile::Strict).unwrap();
        for (i, at) in [1_000u64, 2_000, 3_000].iter().enumerate() {
            j.append(serde_json::to_vec(&record("alice", &format!("a{i}"), *at, Verdict::Permit)).unwrap()).await.unwrap();
        }
        j.append(serde_json::to_vec(&record("bob", "b0", 2_500, Verdict::Permit)).unwrap()).await.unwrap();
        let index = HistoryIndex::load(&path);
        let v = view(index.view("alice", request(), 10_000).0);
        assert_eq!(v.entries.iter().map(|e| e.attempt_id.as_str()).collect::<Vec<_>>(), ["a0", "a1", "a2"]);
        assert!(v.complete && v.scope == HistoryScope::EnforcementPoint && v.as_of_ms == 10_000);
        assert_eq!(view(index.view("bob", request(), 10_000).0).entries.len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_journal_not_yet_written_is_an_empty_history() {
        let v = view(HistoryIndex::load(&temp_journal("absent")).view("alice", request(), 10_000).0);
        assert!(v.entries.is_empty() && v.complete);
    }

    /// A journal that cannot be read gives no history, never an empty one.
    #[test]
    fn an_unreadable_journal_makes_every_history_unavailable() {
        let path = temp_journal("unreadable");
        std::fs::create_dir_all(&path).unwrap(); // a directory where the journal file should be
        let (h, _) = HistoryIndex::load(&path).view("alice", request(), 10_000);
        assert!(matches!(h, History::Unavailable(ref why) if why.contains("could not be read")), "{h:?}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A permit decided on a view is counted only into the history it was decided on.
    #[test]
    fn a_permit_is_admitted_only_at_the_version_it_was_decided_on() {
        let index = HistoryIndex::load(&temp_journal("admit"));
        let (_, v0) = index.view("alice", request(), 10_000);
        let (_, also_v0) = index.view("alice", request(), 10_000);
        let entry = |a: &str| HistoryEntry {
            at_ms: 9_000, attempt_id: a.into(), operation: "tools/call".into(), resource: "tool:post@n1".into(),
            decision: DecisionKind::Permit, execution: Execution::Attempted,
        };
        assert!(index.admit("alice", v0, entry("first")));
        assert!(!index.admit("alice", also_v0, entry("second")), "decided on a history that has since changed");
        let (h, v1) = index.view("alice", request(), 10_000);
        assert_eq!(view(h).entries.len(), 1);
        assert!(index.admit("alice", v1, entry("second")));
        // Taken back when it was not recorded.
        index.retract("alice", "second");
        assert_eq!(view(index.view("alice", request(), 10_000).0).entries.len(), 1);
    }

    /// The record that follows an admitted permit replaces it, and an execution updates it — no double count.
    #[test]
    fn the_journal_record_of_an_admitted_permit_is_not_counted_twice() {
        let index = HistoryIndex::load(&temp_journal("observe"));
        let (_, v) = index.view("alice", request(), 10_000);
        let r = record("alice", "p1", 9_000, Verdict::Permit);
        assert!(index.admit("alice", v, HistoryEntry {
            at_ms: r.at_ms, attempt_id: r.attempt_id.clone(), operation: r.operation.clone(), resource: r.resource.clone(),
            decision: DecisionKind::Permit, execution: Execution::Attempted,
        }));
        index.observe(&r);
        index.observe(&r.as_execution(Execution::Completed));
        let v = view(index.view("alice", request(), 10_000).0);
        assert_eq!(v.entries.len(), 1);
        assert_eq!(v.entries[0].execution, Execution::Completed);
    }

    /// Bounded: an actor keeps at most `PER_ACTOR` records, and a view reaching past one it dropped says so.
    #[test]
    fn a_view_reaching_past_a_dropped_record_is_incomplete() {
        let index = HistoryIndex::load(&temp_journal("cap"));
        for i in 0..(PER_ACTOR as u64 + 5) {
            index.observe(&record("alice", &format!("p{i}"), 100_000 + i, Verdict::Permit));
        }
        let wide = view(index.view("alice", HistoryRequest { window_ms: WINDOW, max_entries: 2 * PER_ACTOR }, 200_000).0);
        assert_eq!(wide.entries.len(), PER_ACTOR);
        assert!(!wide.complete, "the five oldest were dropped and are inside the window");
        // Capped by the request too: the newest it asked for, and incomplete.
        let few = view(index.view("alice", HistoryRequest { window_ms: WINDOW, max_entries: 3 }, 200_000).0);
        assert_eq!(few.entries.len(), 3);
        assert_eq!(few.entries[2].attempt_id, format!("p{}", PER_ACTOR as u64 + 4));
        assert!(!few.complete);
        // A window that ends after the dropped records is complete.
        let recent = view(index.view("alice", HistoryRequest { window_ms: 99_000, max_entries: 2 * PER_ACTOR }, 200_000).0);
        assert!(recent.complete && recent.entries.len() < PER_ACTOR);
    }

    #[test]
    fn a_window_longer_than_the_horizon_is_never_complete() {
        let index = HistoryIndex::load(&temp_journal("horizon"));
        index.observe(&record("alice", "p", 1_000, Verdict::Permit));
        assert!(!view(index.view("alice", HistoryRequest { window_ms: HORIZON_MS + 1, max_entries: 10 }, 2_000).0).complete);
        assert!(!view(index.view("bob", HistoryRequest { window_ms: HORIZON_MS + 1, max_entries: 10 }, 2_000).0).complete);
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
