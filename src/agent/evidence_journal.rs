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
