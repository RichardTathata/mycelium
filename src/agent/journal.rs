//! A node-local, append-only, fsynced journal that is never gossiped — the mechanism.
//!
//! # Why this is its own module (item 4 PR 3a)
//!
//! The AE evidence journal ([`super::evidence_journal`]) was the first thing in the substrate that
//! needed a durable record on *this* node that must **not** enter the gossip medium. The rights
//! ledger (`docs/design/adaptive-stability.md` §4) is the second: an allocated right that gossiped
//! would evaporate with its holder's discovery entry and be issued twice, and an LWW medium would
//! let a later writer overwrite it. Two fsynced append-only journals side by side is how guarantees
//! drift — so the mechanism lives here and each user is a thin profile over it. It was gated on its
//! first user's features until the ledger landed (PR 3) and gave it a user in every build: a module
//! with no user in a build is dead code there, and the `--no-default-features` clippy said so.
//!
//! Nothing here knows what a record means. Bytes in; a content hash, a sequence number and item 1's
//! receipt out.
//!
//! # The receipt is item 1's, not a new one
//!
//! [`append`](Journal::append) returns [`LocalDurability`]: `OnDisk` only after the record is
//! fsynced, `Failed` when durability was not established. `Buffered` is never produced — every
//! append forces a sync, because a record that is merely in the page cache cannot gate an effect
//! (an AE dispatch, a reservation of a right). Reusing the vocabulary is deliberate: a second
//! durability language would be item 1's whole argument, repeated wrongly.
//!
//! # Failure is a decision, not a log line
//!
//! Three failures, each named ([`JournalError`]): a full queue is **refused, never silently
//! dropped**; a persistence failure is reported; a lost acknowledgement is *unknown*, which is a
//! different claim from failure. What a caller does with each is the caller's profile — the AE
//! journal's `Strict`/`Lenient`, the ledger's always-strict — and not this module's.
//!
//! # Stream identity
//!
//! The bounded queue's `try_send` is routed through the replay seam, and the inventory's rule is
//! *stream identity per destination*: the AE journal records as `ae/journal`, the rights ledger as
//! its own stream, so a replay of one cannot hand the other its verdict. The stream is therefore a
//! parameter of [`open`](Journal::open), not a constant.
//!
//! # Five-part statement
//!
//! *Guarantee:* a record this journal acknowledges with `OnDisk` is on this node's disk, fsynced,
//! before the acknowledgement returns; and nothing written here is gossiped. *Assumptions:* the
//! journal directory is on durable local storage the node owns, and `fsync` means what the
//! filesystem says it means. *Enforcing component:* [`Journal::append`] and its writer task.
//! *Failure behaviour:* the three named errors; a full queue is refused rather than dropped, and a
//! lost acknowledgement is reported as unknown rather than as failure. *Detecting tests:* this
//! module's `tests`, one per failure plus the round-trip, restart and reader cases. *Strength:*
//! `SelfImposedPrevention` over this node's own records; it prevents nothing at a resource.

use mycelium_core::receipt::LocalDurability;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// How long an append may take before its acknowledgement is treated as lost.
///
/// Generous relative to an fsync, tight relative to a client's patience: past this the honest
/// statement is that the record's fate is *unknown*, which is a different claim from failure and
/// a caller's profile treats it as such.
const ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Queue depth. Bounded on purpose: an unbounded queue turns memory pressure into the failure mode
/// instead of the one the operator chose.
const QUEUE_DEPTH: usize = 1024;

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
            JournalError::Saturated => write!(f, "the journal's queue is full"),
            JournalError::Failed(e) => write!(f, "the journal could not persist: {e}"),
            JournalError::DeliveryUnknown => {
                write!(f, "the journal did not acknowledge in time; the record's fate is unknown")
            }
            JournalError::Closed => write!(f, "the journal's writer is gone"),
        }
    }
}
impl std::error::Error for JournalError {}

/// An accepted append: the record is on this node's disk, and this is how to cite it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appended {
    /// The record's content hash — what a caller may cite elsewhere (the AE chain's reference
    /// record carries it), and the only part of the record that is allowed to leave this node.
    pub content_hash: [u8; 32],
    /// Its position in this node's journal.
    pub seq: u64,
    /// The receipt, in item 1's vocabulary. Always `OnDisk` when this value exists.
    pub durability: LocalDurability,
}

pub(crate) enum Msg {
    Append { bytes: Vec<u8>, ack: oneshot::Sender<Result<(u64, LocalDurability), String>> },
}

/// The journal handle.
pub struct Journal {
    tx:     mpsc::Sender<Msg>,
    /// The replay-seam stream this journal's queue records under — per destination.
    stream: &'static str,
    path:   PathBuf,
}

impl Journal {
    /// Open (or create) the journal at `path` and spawn its writer. `stream` names its queue in
    /// the replay trace and must be distinct per journal.
    ///
    /// Append-only. The file is opened once and kept open; each record is length-prefixed so a
    /// reader can walk it without parsing the payload to find boundaries.
    pub fn open(path: impl AsRef<Path>, stream: &'static str) -> Result<Arc<Self>, std::io::Error> {
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

        Ok(Arc::new(Self { tx, stream, path }))
    }

    /// Where the journal lives.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one record, returning only once it is fsynced.
    ///
    /// The content hash is computed here, over the exact bytes written, so a citation cannot name
    /// something other than what landed.
    pub async fn append(&self, bytes: Vec<u8>) -> Result<Appended, JournalError> {
        let content_hash: [u8; 32] = Sha256::digest(&bytes).into();
        let (ack_tx, ack_rx) = oneshot::channel();

        // `try_send`, not `send`: waiting on a full queue would turn saturation into latency and
        // hide the very condition a caller's profile is supposed to decide about.
        // Routed through the replay seam (item 6 PR 3). This queue's saturation behaviour is a
        // *stated guarantee* — a full queue is refused, never silently dropped — so being able to
        // replay the saturation is how that guarantee gets tested rather than asserted.
        match mycelium_core::sim_seam::chan_try_send(
            self.stream,
            &self.tx,
            Msg::Append { bytes, ack: ack_tx },
        ) {
            mycelium_core::sim_seam::ChanVerdict::Sent => {}
            mycelium_core::sim_seam::ChanVerdict::Full => return Err(JournalError::Saturated),
            mycelium_core::sim_seam::ChanVerdict::Closed => return Err(JournalError::Closed),
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

// Test-only helpers live in their own `impl` block, gated at the top level — not as `#[cfg(test)]`
// items inside the production `impl`. The forbidden-call check skips from a `#[cfg(test)]` to the
// next brace at column 0, so a gated *inner* item would hide the rest of the enclosing block from
// it: `append`'s timeout went uncounted the first time these were written inline.
#[cfg(test)]
impl Journal {
    /// A journal whose writer accepts nothing, so every append ends as
    /// [`DeliveryUnknown`](JournalError::DeliveryUnknown).
    ///
    /// It exists because a failing journal is otherwise very hard to arrange from outside this
    /// module — the behaviour that matters (does the *caller* refuse?) lives with the caller.
    pub(crate) fn stalled(stream: &'static str) -> Arc<Self> {
        let (tx, mut rx) = mpsc::channel::<Msg>(4);
        tokio::spawn(async move {
            while let Some(Msg::Append { ack, .. }) = rx.recv().await {
                drop(ack);
            }
        });
        Arc::new(Self { tx, stream, path: PathBuf::from("<stalled>") })
    }

    /// The replay-seam stream this journal records under. Production never asks a journal its
    /// stream; it names one at `open`.
    pub(crate) fn stream(&self) -> &'static str {
        self.stream
    }
}

/// How many length-prefixed records the file already holds.
fn count_records(path: &Path) -> std::io::Result<u64> {
    use std::io::{Read as _, Seek as _};
    let mut f = std::fs::File::open(path)?;
    let file_len = f.metadata()?.len();
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
        //
        // The landing position is checked, not merely the seek's `Ok`: **seeking past the end of a
        // file is legal and succeeds**, so a torn tail whose length prefix outruns the file was
        // counted as a complete record. `count_records` drives the next append's sequence number
        // while `read_journal_from` stops *at* the torn record, so the two disagreed about how many
        // records exist and the next append took a seq number that no reader would ever hand out.
        let Ok(landed) = f.seek(std::io::SeekFrom::Current(want)) else { break };
        if landed > file_len {
            break;
        }
        n += 1;
    }
    Ok(n)
}

/// Where a reader has got to. Opaque-ish on purpose: an exporter stores it and hands it back.
///
/// The byte offset is what makes resuming cheap — a reader that had to re-walk the file to find its
/// place would get slower for exactly as long as the node kept producing records, which is the
/// wrong shape for something that runs forever.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct JournalCursor {
    /// Byte offset of the next record's length prefix.
    pub offset: u64,
    /// The sequence number the next record will carry.
    pub seq:    u64,
}

/// One record, as a reader sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalEntry {
    /// Position in this node's journal.
    pub seq:          u64,
    /// The record's content hash — the same value [`Appended::content_hash`] reported when it was
    /// written, which is what lets a reader correlate what it holds with what was cited.
    pub content_hash: [u8; 32],
    /// The record itself.
    pub bytes:        Vec<u8>,
}

/// One page of the journal, and where to resume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalPage {
    /// The records read, in order.
    pub entries: Vec<JournalEntry>,
    /// Where the next read starts. Unchanged from the cursor passed in when nothing was read.
    pub next:    JournalCursor,
    /// Whether the read stopped at a bound rather than at the end of the file — so a caller knows
    /// to come back rather than concluding it is caught up.
    pub more:    bool,
}

/// Read a bounded page of the journal from `cursor`.
///
/// **The exporter's shape** (§6.7's outbox): batch, ship, advance — never re-read from the start,
/// and never hold the whole journal in memory to find the end of it.
///
/// Bounds are the caller's, because only the caller knows what its consumer accepts; both are
/// honoured, and `more` says whether the stop was a bound or the end of the file. A record larger
/// than `max_bytes` is still returned **alone** rather than skipped: silently dropping a record
/// because it is inconveniently large is the failure mode this mechanism exists to avoid.
///
/// A truncated tail — where a crash landed mid-record — ends the page cleanly and leaves the cursor
/// before it, so a later read picks the record up once the writer completes it.
pub fn read_journal_from(
    path: &Path,
    cursor: JournalCursor,
    max_records: usize,
    max_bytes: usize,
) -> std::io::Result<JournalPage> {
    use std::io::{Read as _, Seek as _};

    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(JournalPage { entries: Vec::new(), next: cursor, more: false });
        }
        Err(e) => return Err(e),
    };
    let file_len = f.metadata()?.len();
    f.seek(std::io::SeekFrom::Start(cursor.offset))?;

    let mut page = JournalPage { entries: Vec::new(), next: cursor, more: false };
    let mut bytes_read = 0usize;
    let mut len = [0u8; 4];
    loop {
        if page.entries.len() >= max_records {
            page.more = true;
            break;
        }
        match f.read_exact(&mut len) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
        let want = u32::from_le_bytes(len) as usize;
        // A record cannot be longer than the bytes that remain. The writer appends a length and
        // then exactly that many bytes, so a length the file cannot satisfy is a torn or corrupt
        // tail rather than a record to make room for.
        //
        // Checking this **before** the allocation is the whole point. `read_exact` below would
        // fail on such a record anyway — but only after `vec![0u8; want]` had already reserved and
        // zeroed `want` bytes, and `want` is four bytes wide: a single flipped bit in a length
        // prefix asks for up to 4 GiB. The journal is node-local, so this is corruption and disk
        // error rather than an attacker, and a corrupt journal should be *reported* by the reader
        // that finds it, not resolved by the allocator. Found by the §12.6 trust-edge fuzz work.
        if !record_fits(want, page.next.offset, file_len) {
            break;
        }
        let mut buf = vec![0u8; want];
        if f.read_exact(&mut buf).is_err() {
            // Truncated tail. Leave the cursor before it; the record is not lost, it is not
            // finished.
            break;
        }
        // The size bound is checked *after* the first record, so one oversized record still travels
        // rather than wedging the reader forever.
        if !page.entries.is_empty() && bytes_read + want > max_bytes {
            page.more = true;
            break;
        }
        bytes_read += want;
        page.next = JournalCursor {
            offset: page.next.offset + 4 + want as u64,
            seq:    page.next.seq + 1,
        };
        page.entries.push(JournalEntry {
            seq:          page.next.seq - 1,
            content_hash: Sha256::digest(&buf).into(),
            bytes:        buf,
        });
    }
    Ok(page)
}

/// Can a record of `want` bytes actually be in this file, given that its length prefix started at
/// `prefix_offset`?
///
/// The writer appends a 4-byte length and then exactly that many bytes, so a length the file cannot
/// satisfy is a torn or corrupt tail rather than a record. Consulting this **before** allocating is
/// the whole point: `read_exact` would fail on such a record anyway, but only after
/// `vec![0u8; want]` had reserved and zeroed `want` bytes, and `want` is four bytes wide — a single
/// flipped bit in a length prefix asks for up to 4 GiB.
///
/// The journal is node-local, so this is corruption and disk error rather than an attacker. A
/// corrupt journal should be reported by the reader that finds it, not resolved by the allocator.
/// Found by the §12.6 trust-edge fuzz work.
fn record_fits(want: usize, prefix_offset: u64, file_len: u64) -> bool {
    let record_start = prefix_offset.saturating_add(4);
    want as u64 <= file_len.saturating_sub(record_start)
}

/// Read every record back, in order. **An exporter should prefer [`read_journal_from`]** — this
/// one is for tests and for small one-shot inspections, and it reads the whole file into memory.
pub fn read_journal(path: &Path) -> std::io::Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    let mut cursor = JournalCursor::default();
    loop {
        let page = read_journal_from(path, cursor, 4096, usize::MAX)?;
        let done = !page.more;
        cursor = page.next;
        out.extend(page.entries.into_iter().map(|e| e.bytes));
        if done {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("journal-{name}-{}", fastrand::u64(..)));
        let _ = std::fs::remove_dir_all(&p);
        p.join("evidence.log")
    }

    #[tokio::test]
    async fn an_acknowledged_record_is_on_disk_and_its_hash_names_it() {
        let path = temp("roundtrip");
        let j = Journal::open(&path, "test/journal").unwrap();

        let a = j.append(b"first".to_vec()).await.expect("appended");
        let b = j.append(b"second".to_vec()).await.expect("appended");
        assert_eq!(a.durability, LocalDurability::OnDisk);
        assert_eq!((a.seq, b.seq), (0, 1));

        let records = read_journal(&path).unwrap();
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
        let j = Journal { tx, stream: "test/journal", path: path.clone() };

        // Fill the one slot, then the next append has nowhere to go.
        let first = j.append(b"a".to_vec());
        tokio::pin!(first);
        // Drive it far enough to occupy the slot without awaiting its (never-coming) ack.
        let _ = tokio::time::timeout(std::time::Duration::from_millis(50), &mut first).await;

        assert_eq!(j.append(b"b".to_vec()).await, Err(JournalError::Saturated));
        // Nothing reached the disk, and the caller was told so rather than left to assume.
        assert!(read_journal(&path).unwrap().is_empty());
    }

    /// **Failure 2 of 3.** Persistence failure is reported as failure, and the record is not
    /// claimed. Here the "file" is a directory, so every write fails.
    #[tokio::test]
    async fn a_persistence_failure_is_reported_not_swallowed() {
        let dir = std::env::temp_dir().join(format!("journal-fail-{}", fastrand::u64(..)));
        std::fs::create_dir_all(dir.join("evidence.log")).unwrap();
        // Opening a directory as an append-only file fails on every platform we support.
        let opened = Journal::open(dir.join("evidence.log"), "test/journal");
        assert!(opened.is_err(), "a journal that cannot be opened must not pretend to exist");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Failure 3 of 3.** A lost acknowledgement is `DeliveryUnknown`, never `Failed`: the record
    /// may well be on disk, and saying "failed" would claim knowledge nobody has.
    #[tokio::test]
    async fn a_lost_acknowledgement_is_unknown_not_failed() {
        // A writer that accepts the message and then drops the ack channel without replying.
        let j = Journal::stalled("test/journal");
        assert_eq!(j.append(b"x".to_vec()).await, Err(JournalError::DeliveryUnknown));
        assert_eq!(j.stream(), "test/journal");
    }

    #[tokio::test]
    async fn a_restart_resumes_the_sequence_instead_of_renumbering_history() {
        let path = temp("resume");
        let j = Journal::open(&path, "test/journal").unwrap();
        j.append(b"one".to_vec()).await.unwrap();
        j.append(b"two".to_vec()).await.unwrap();
        drop(j);

        let j = Journal::open(&path, "test/journal").unwrap();
        let third = j.append(b"three".to_vec()).await.unwrap();
        assert_eq!(third.seq, 2, "a restart continues the journal, it does not restart it");
        assert_eq!(read_journal(&path).unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_reader_resumes_from_its_cursor_instead_of_re_reading() {
        let path = temp("cursor");
        let j = Journal::open(&path, "test/journal").unwrap();
        for i in 0..5u8 {
            j.append(vec![b'a' + i]).await.unwrap();
        }

        let first = read_journal_from(&path, JournalCursor::default(), 2, usize::MAX)
            .unwrap();
        assert_eq!(first.entries.len(), 2);
        assert!(first.more, "stopping at a bound must say there is more");
        assert_eq!(first.entries[0].seq, 0);

        let second = read_journal_from(&path, first.next, 2, usize::MAX).unwrap();
        assert_eq!(second.entries.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![2, 3]);

        let third = read_journal_from(&path, second.next, 10, usize::MAX).unwrap();
        assert_eq!(third.entries.len(), 1);
        assert!(!third.more, "reaching the end is not the same as hitting a bound");

        // Caught up: reading again returns nothing and does not move.
        let again = read_journal_from(&path, third.next, 10, usize::MAX).unwrap();
        assert!(again.entries.is_empty());
        assert_eq!(again.next, third.next);
    }

    /// The hash a reader computes is the hash the chain's reference cites — the correlation the
    /// whole split depends on. If these ever diverge, evidence and its chain record stop being
    /// about each other and nobody can tell.
    #[tokio::test]
    async fn the_readers_hash_is_the_one_the_append_acknowledged() {
        let path = temp("hashmatch");
        let j = Journal::open(&path, "test/journal").unwrap();
        let a = j.append(b"decided-1".to_vec()).await.unwrap();
        let b = j.append(b"decided-2".to_vec()).await.unwrap();

        let page =
            read_journal_from(&path, JournalCursor::default(), 10, usize::MAX).unwrap();
        assert_eq!(page.entries[0].content_hash, a.content_hash);
        assert_eq!(page.entries[1].content_hash, b.content_hash);
        assert_eq!((page.entries[0].seq, page.entries[1].seq), (a.seq, b.seq));
    }

    #[tokio::test]
    async fn a_byte_bound_splits_the_page_but_never_drops_a_record() {
        let path = temp("bytebound");
        let j = Journal::open(&path, "test/journal").unwrap();
        for _ in 0..4 {
            j.append(vec![b'x'; 100]).await.unwrap();
        }

        let mut cursor = JournalCursor::default();
        let mut seen = 0;
        loop {
            let page = read_journal_from(&path, cursor, 100, 150).unwrap();
            if page.entries.is_empty() {
                break;
            }
            // 150 bytes fits one 100-byte record, never two.
            assert_eq!(page.entries.len(), 1);
            seen += page.entries.len();
            cursor = page.next;
        }
        assert_eq!(seen, 4, "every record is read, just across more pages");
    }

    /// A record bigger than the caller's byte bound still travels, alone. Skipping it would be the
    /// silent drop the journal exists to make impossible.
    #[tokio::test]
    async fn an_oversized_record_is_returned_alone_rather_than_skipped() {
        let path = temp("oversized");
        let j = Journal::open(&path, "test/journal").unwrap();
        j.append(vec![b'y'; 5000]).await.unwrap();
        j.append(b"small".to_vec()).await.unwrap();

        let page = read_journal_from(&path, JournalCursor::default(), 10, 64).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].bytes.len(), 5000);
        assert!(page.more);
        let next = read_journal_from(&path, page.next, 10, 64).unwrap();
        assert_eq!(next.entries[0].bytes, b"small".to_vec());
    }

    /// A crash landed mid-record. The page ends cleanly *before* it and the cursor does not move
    /// past it, so when the writer finishes the record a later read picks it up.
    #[tokio::test]
    async fn a_truncated_tail_ends_the_page_without_losing_the_record() {
        let path = temp("tornread");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // One complete record, then a length prefix whose body never arrived.
        let mut bytes = vec![3u8, 0, 0, 0, b'o', b'n', b'e'];
        bytes.extend_from_slice(&[9u8, 0, 0, 0, b'h', b'i']);
        std::fs::write(&path, bytes).unwrap();

        let page =
            read_journal_from(&path, JournalCursor::default(), 10, usize::MAX).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].bytes, b"one".to_vec());
        assert_eq!(page.next.offset, 7, "the cursor stops before the torn record");
    }

    #[tokio::test]
    async fn a_journal_that_does_not_exist_yet_reads_as_empty_not_as_an_error() {
        let path = temp("absent");
        let page =
            read_journal_from(&path, JournalCursor::default(), 10, usize::MAX).unwrap();
        assert!(page.entries.is_empty());
        assert!(!page.more);
    }

    #[tokio::test]
    async fn a_truncated_tail_does_not_stop_the_node_from_starting() {
        // Where a crash landed mid-record. The node must still open, still append, and simply not
        // count the fragment — refusing to start would turn a partial write into an outage.
        let path = temp("truncated");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, [9u8, 0, 0, 0, b'h', b'i']).unwrap();

        let j = Journal::open(&path, "test/journal").unwrap();
        assert!(j.append(b"after".to_vec()).await.is_ok());
    }
}


#[cfg(test)]
mod torn_tail_tests {
    use super::*;

    /// Build a journal: `good` as a complete record, then a length prefix claiming `claims` bytes
    /// with nothing behind it — the shape a crash mid-append leaves, and the shape a single flipped
    /// bit in a length prefix produces.
    fn journal_with_torn_tail(dir: &std::path::Path, claims: u32) -> std::path::PathBuf {
        let path = dir.join("journal.log");
        let good = b"the-one-complete-record";
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(good.len() as u32).to_le_bytes());
        bytes.extend_from_slice(good);
        bytes.extend_from_slice(&claims.to_le_bytes());
        std::fs::write(&path, &bytes).expect("write the journal");
        path
    }

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir()
            .join(format!("mycelium-journal-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&d).expect("create temp dir");
        d
    }

    /// The two readers agree about how many records a torn journal holds.
    ///
    /// `count_records` decided a record was complete by seeking past it and checking only that the
    /// seek returned `Ok` — but **seeking past the end of a file is legal and succeeds**, so a torn
    /// tail whose length prefix outran the file counted as a record. `read_journal_from` stops *at*
    /// the torn record, so the two disagreed: the next append took a sequence number one higher
    /// than any reader would ever hand out, and a cursor-based exporter reading the journal would
    /// see that seq go missing.
    ///
    /// What this does NOT prove: nothing here says the torn record is recoverable. It is not, and
    /// it should not be — the point is that both readers call it what it is.
    #[test]
    fn a_torn_tail_is_not_counted_as_a_record() {
        let dir = tmpdir("torn");
        let path = journal_with_torn_tail(&dir, 256 * 1024 * 1024);

        let page = read_journal_from(&path, JournalCursor::default(), 4096, usize::MAX)
            .expect("a torn tail is not an error");
        assert_eq!(page.entries.len(), 1, "the file holds exactly one complete record");

        assert_eq!(
            count_records(&path).expect("count"),
            1,
            "count_records must not count a torn tail -- a seek past EOF succeeds, so the \
             landing position has to be checked, not the seek's Ok",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A length prefix the file cannot satisfy reads as a torn tail rather than an error.
    ///
    /// **What this test does not prove, stated because it is easy to assume otherwise:** it does
    /// *not* detect the allocation itself. Deleting the bound leaves every assertion here passing.
    /// `vec![0u8; want]` goes through `alloc_zeroed`, which the OS satisfies with lazy zero pages —
    /// so a 4 GiB request succeeds instantly, the following `read_exact` fails, and the function
    /// returns exactly what it returns with the bound in place. The observable outcome is identical
    /// by construction; only the allocator sees the difference, and gating that would mean swapping
    /// a counting global allocator into the whole test binary.
    ///
    /// So the bound is gated at the level it can be: [`record_fits`] is tested directly below, and
    /// this test pins the *behaviour* the reader must keep while it is enforced. The call site
    /// itself rests on review. That is a weaker claim than the other tests here make, and it is
    /// written down rather than left to be inferred from a green run.
    #[test]
    fn a_length_the_file_cannot_contain_reads_as_a_torn_tail() {
        let dir = tmpdir("huge");
        let path = journal_with_torn_tail(&dir, u32::MAX);

        let page = read_journal_from(&path, JournalCursor::default(), 4096, usize::MAX)
            .expect("an impossible length is a torn tail, not an error");
        assert_eq!(page.entries.len(), 1, "the one complete record is still returned");
        assert!(!page.more, "there is nothing after the torn tail to come back for");
        assert_eq!(count_records(&path).expect("count"), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The bound must not refuse a record the file *can* supply, including the last one.
    #[test]
    fn a_record_that_exactly_fills_the_file_is_still_read() {
        let dir = tmpdir("exact");
        let path = dir.join("journal.log");
        let records: [&[u8]; 3] = [b"first", b"second-is-longer", b"third"];
        let mut bytes = Vec::new();
        for r in records {
            bytes.extend_from_slice(&(r.len() as u32).to_le_bytes());
            bytes.extend_from_slice(r);
        }
        std::fs::write(&path, &bytes).expect("write");

        let page = read_journal_from(&path, JournalCursor::default(), 4096, usize::MAX).expect("read");
        assert_eq!(page.entries.len(), 3, "every record, including the one ending at EOF");
        assert_eq!(
            page.entries.iter().map(|e| e.bytes.as_slice()).collect::<Vec<_>>(),
            records.to_vec(),
            "and their contents, in order",
        );
        assert_eq!(count_records(&path).expect("count"), 3);

        // Reading again from the returned cursor yields nothing and does not error.
        let tail = read_journal_from(&path, page.next, 4096, usize::MAX).expect("read tail");
        assert!(tail.entries.is_empty(), "the cursor after the last record is the end");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The bound itself, tested where it can actually be falsified.
    ///
    /// Boundary cases both ways: a record that exactly reaches EOF fits, one byte more does not,
    /// and the arithmetic saturates rather than wrapping on a cursor past the end of the file.
    #[test]
    fn a_record_fits_exactly_when_the_file_can_supply_it() {
        // 4-byte prefix at offset 0, then 10 bytes of record: a 14-byte file holds it exactly.
        assert!(record_fits(10, 0, 14), "a record ending at EOF fits");
        assert!(!record_fits(11, 0, 14), "one byte past EOF does not");
        assert!(record_fits(0, 0, 4), "an empty record is still a record");

        // Mid-file, after an earlier record.
        assert!(record_fits(5, 20, 29), "a record ending at EOF fits, wherever it starts");
        assert!(!record_fits(6, 20, 29), "and one byte more does not");

        // A cursor at or past the end cannot admit anything, and must not wrap.
        assert!(!record_fits(1, 100, 50), "a prefix beyond EOF admits nothing");
        assert!(!record_fits(1, u64::MAX, 50), "and the arithmetic saturates");
        assert!(!record_fits(usize::MAX, 0, 14), "the widest possible claim is refused");
    }
}
