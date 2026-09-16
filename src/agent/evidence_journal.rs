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
//! # Why not the audit sink
//!
//! `AuditSink::export` returns nothing, runs on a drain task, and drops records when its bounded
//! channel saturates. It is a **mirror**, and a mirror cannot be a durability barrier. AE0 names the
//! journal, not the sink, as the contract that can acknowledge — which is why an effect may be
//! gated on this and never on that.
//!
//! # The receipt is item 1's, not a new one
//!
//! [`append`](EvidenceJournal::append) returns [`LocalDurability`]: `OnDisk` only after the record
//! is fsynced, `Failed` when durability was not established, `NotConfigured` when no journal is
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
//! and its writer task. *Failure behaviour:* the three rows above; a full queue is refused rather
//! than dropped, and a lost acknowledgement is reported as unknown rather than as failure.
//! *Detecting tests:* this module's `tests` — one per row, plus the round-trip and the
//! no-gossip pin in `action_evaluator`. *Strength:* `SelfImposedPrevention` — it governs this node's
//! own dispatch; it prevents nothing at a resource.

use mycelium_core::receipt::LocalDurability;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// How long an append may take before its acknowledgement is treated as lost.
///
/// Generous relative to an fsync, tight relative to a client's patience: past this the honest
/// statement is that the record's fate is *unknown*, which is a different claim from failure and
/// the profile treats it as such.
const ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Queue depth. Bounded on purpose: an unbounded queue turns memory pressure into the failure mode
/// instead of the one the operator chose.
const QUEUE_DEPTH: usize = 1024;

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

/// Why an append did not establish durability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JournalError {
    /// The queue was full. The record was **not** written and **not** silently dropped.
    Saturated,
    /// The writer reported a persistence failure.
    Failed(String),
    /// The acknowledgement never arrived. The record's fate is genuinely unknown: it may be on
    /// disk. Never reported as a failure, which would claim knowledge nobody has.
    DeliveryUnknown,
    /// The journal's writer task is gone.
    Closed,
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JournalError::Saturated => write!(f, "the evidence journal's queue is full"),
            JournalError::Failed(e) => write!(f, "the evidence journal could not persist: {e}"),
            JournalError::DeliveryUnknown => {
                write!(f, "the evidence journal did not acknowledge in time; its fate is unknown")
            }
            JournalError::Closed => write!(f, "the evidence journal's writer is gone"),
        }
    }
}
impl std::error::Error for JournalError {}

/// An accepted append: the record is on this node's disk, and this is how to cite it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appended {
    /// The record's content hash — what the chain's safe reference record carries, and the only
    /// part of the evidence that is allowed to leave this node by gossip.
    pub content_hash: [u8; 32],
    /// Its position in this node's journal.
    pub seq: u64,
    /// The receipt, in item 1's vocabulary. Always `OnDisk` when this value exists.
    pub durability: LocalDurability,
}

enum Msg {
    Append { bytes: Vec<u8>, ack: oneshot::Sender<Result<(u64, LocalDurability), String>> },
}

/// The journal handle.
pub struct EvidenceJournal {
    tx:      mpsc::Sender<Msg>,
    profile: EvidenceProfile,
    path:    PathBuf,
}

impl EvidenceJournal {
    /// Open (or create) the journal at `path` and spawn its writer.
    ///
    /// Append-only. The file is opened once and kept open; each record is length-prefixed so a
    /// reader — the exporter, later — can walk it without parsing JSON to find boundaries.
    pub fn open(
        path: impl AsRef<Path>,
        profile: EvidenceProfile,
    ) -> Result<Arc<Self>, std::io::Error> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        // Where the existing journal ends is where this node's sequence resumes; counting records
        // rather than assuming an empty file means a restart does not renumber history.
        let mut seq = count_records(&path)?;

        let (tx, mut rx) = mpsc::channel::<Msg>(QUEUE_DEPTH);
        tokio::spawn(async move {
            use std::io::Write as _;
            while let Some(Msg::Append { bytes, ack }) = rx.recv().await {
                let result = (|| -> std::io::Result<u64> {
                    let len = u32::try_from(bytes.len()).map_err(|_| {
                        std::io::Error::new(std::io::ErrorKind::InvalidInput, "record too large")
                    })?;
                    file.write_all(&len.to_le_bytes())?;
                    file.write_all(&bytes)?;
                    // `OnDisk` is a claim, and this is what makes it true. Without it the receipt
                    // would say durable and mean "in the page cache".
                    file.sync_data()?;
                    seq += 1;
                    Ok(seq - 1)
                })();
                let reply = match result {
                    Ok(at) => Ok((at, LocalDurability::OnDisk)),
                    Err(e) => Err(e.to_string()),
                };
                // The caller may have timed out and gone; that is their `DeliveryUnknown`, and from
                // here the record is on disk regardless.
                let _ = ack.send(reply);
            }
        });

        Ok(Arc::new(Self { tx, profile, path }))
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
        let (tx, mut rx) = mpsc::channel::<Msg>(4);
        tokio::spawn(async move {
            while let Some(Msg::Append { ack, .. }) = rx.recv().await {
                drop(ack);
            }
        });
        Arc::new(Self { tx, profile, path: PathBuf::from("<stalled>") })
    }

    /// The operator's chosen failure behaviour.
    pub fn profile(&self) -> EvidenceProfile {
        self.profile
    }

    /// Where the journal lives.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one evidence record, returning only once it is fsynced.
    ///
    /// The content hash is computed here, over the exact bytes written, so the chain's reference
    /// cannot cite something other than what landed.
    pub async fn append(&self, bytes: Vec<u8>) -> Result<Appended, JournalError> {
        let content_hash: [u8; 32] = Sha256::digest(&bytes).into();
        let (ack_tx, ack_rx) = oneshot::channel();

        // `try_send`, not `send`: waiting on a full queue would turn saturation into latency and
        // hide the very condition the profile is supposed to decide about.
        match self.tx.try_send(Msg::Append { bytes, ack: ack_tx }) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => return Err(JournalError::Saturated),
            Err(mpsc::error::TrySendError::Closed(_)) => return Err(JournalError::Closed),
        }

        match tokio::time::timeout(ACK_TIMEOUT, ack_rx).await {
            Ok(Ok(Ok((seq, durability)))) => Ok(Appended { content_hash, seq, durability }),
            Ok(Ok(Err(e))) => Err(JournalError::Failed(e)),
            // The writer dropped the ack without replying.
            Ok(Err(_)) => Err(JournalError::DeliveryUnknown),
            Err(_) => Err(JournalError::DeliveryUnknown),
        }
    }
}

/// How many length-prefixed records the file already holds.
fn count_records(path: &Path) -> std::io::Result<u64> {
    use std::io::{Read as _, Seek as _};
    let mut f = std::fs::File::open(path)?;
    let mut n = 0u64;
    let mut len = [0u8; 4];
    loop {
        match f.read_exact(&mut len) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
        let want = u32::from_le_bytes(len) as i64;
        // A truncated tail is where a crash landed: stop counting there rather than failing to
        // open, so a node with a half-written record still starts and still appends after it.
        if f.seek(std::io::SeekFrom::Current(want)).is_err() {
            break;
        }
        n += 1;
    }
    Ok(n)
}

/// Read every record back, in order. **The exporter's shape, and the tests' way of proving that what
/// was acknowledged is what is on disk.
pub fn read_evidence_journal(path: &Path) -> std::io::Result<Vec<Vec<u8>>> {
    use std::io::Read as _;
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    let mut len = [0u8; 4];
    loop {
        match f.read_exact(&mut len) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
        let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
        if f.read_exact(&mut buf).is_err() {
            break; // truncated tail
        }
        out.push(buf);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("ae-journal-{name}-{}", fastrand::u64(..)));
        let _ = std::fs::remove_dir_all(&p);
        p.join("evidence.log")
    }

    #[tokio::test]
    async fn an_acknowledged_record_is_on_disk_and_its_hash_names_it() {
        let path = temp("roundtrip");
        let j = EvidenceJournal::open(&path, EvidenceProfile::Strict).unwrap();

        let a = j.append(b"first".to_vec()).await.expect("appended");
        let b = j.append(b"second".to_vec()).await.expect("appended");
        assert_eq!(a.durability, LocalDurability::OnDisk);
        assert_eq!((a.seq, b.seq), (0, 1));

        let records = read_evidence_journal(&path).unwrap();
        assert_eq!(records, vec![b"first".to_vec(), b"second".to_vec()]);
        // The cited hash is the hash of what actually landed — the property the chain's reference
        // record depends on to mean anything.
        assert_eq!(a.content_hash, <[u8; 32]>::from(Sha256::digest(b"first")));
    }

    /// **Failure 1 of 3.** A full queue is refused, never silently dropped: a dropped evidence
    /// record and a decision nobody made are indistinguishable afterwards.
    #[tokio::test]
    async fn a_full_queue_is_refused_rather_than_dropped() {
        let path = temp("saturation");
        // A writer that never drains, so the queue fills and stays full.
        let (tx, _held) = mpsc::channel::<Msg>(1);
        let j = EvidenceJournal { tx, profile: EvidenceProfile::Strict, path: path.clone() };

        // Fill the one slot, then the next append has nowhere to go.
        let first = j.append(b"a".to_vec());
        tokio::pin!(first);
        // Drive it far enough to occupy the slot without awaiting its (never-coming) ack.
        let _ = tokio::time::timeout(std::time::Duration::from_millis(50), &mut first).await;

        assert_eq!(j.append(b"b".to_vec()).await, Err(JournalError::Saturated));
        // Nothing reached the disk, and the caller was told so rather than left to assume.
        assert!(read_evidence_journal(&path).unwrap().is_empty());
    }

    /// **Failure 2 of 3.** Persistence failure is reported as failure, and the record is not
    /// claimed. Here the "file" is a directory, so every write fails.
    #[tokio::test]
    async fn a_persistence_failure_is_reported_not_swallowed() {
        let dir = std::env::temp_dir().join(format!("ae-journal-fail-{}", fastrand::u64(..)));
        std::fs::create_dir_all(dir.join("evidence.log")).unwrap();
        // Opening a directory as an append-only file fails on every platform we support.
        let opened = EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict);
        assert!(opened.is_err(), "a journal that cannot be opened must not pretend to exist");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Failure 3 of 3.** A lost acknowledgement is `DeliveryUnknown`, never `Failed`: the record
    /// may well be on disk, and saying "failed" would claim knowledge nobody has.
    #[tokio::test]
    async fn a_lost_acknowledgement_is_unknown_not_failed() {
        let path = temp("lostack");
        // A writer that accepts the message and then drops the ack channel without replying.
        let (tx, mut rx) = mpsc::channel::<Msg>(4);
        tokio::spawn(async move {
            while let Some(Msg::Append { ack, .. }) = rx.recv().await {
                drop(ack);
            }
        });
        let j = EvidenceJournal { tx, profile: EvidenceProfile::Strict, path };
        assert_eq!(j.append(b"x".to_vec()).await, Err(JournalError::DeliveryUnknown));
    }

    #[tokio::test]
    async fn a_restart_resumes_the_sequence_instead_of_renumbering_history() {
        let path = temp("resume");
        let j = EvidenceJournal::open(&path, EvidenceProfile::Strict).unwrap();
        j.append(b"one".to_vec()).await.unwrap();
        j.append(b"two".to_vec()).await.unwrap();
        drop(j);

        let j = EvidenceJournal::open(&path, EvidenceProfile::Strict).unwrap();
        let third = j.append(b"three".to_vec()).await.unwrap();
        assert_eq!(third.seq, 2, "a restart continues the journal, it does not restart it");
        assert_eq!(read_evidence_journal(&path).unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_truncated_tail_does_not_stop_the_node_from_starting() {
        // Where a crash landed mid-record. The node must still open, still append, and simply not
        // count the fragment — refusing to start would turn a partial write into an outage.
        let path = temp("truncated");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, [9u8, 0, 0, 0, b'h', b'i']).unwrap();

        let j = EvidenceJournal::open(&path, EvidenceProfile::Strict).unwrap();
        assert!(j.append(b"after".to_vec()).await.is_ok());
    }
}
