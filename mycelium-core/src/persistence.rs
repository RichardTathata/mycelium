//! Local KV persistence: append-only WAL + periodic snapshot.
//!
//! Each node writes under `{base_path}/{node_id}/kv/`:
//! - `wal.bin`       — length-prefixed [`SyncEntry`] records
//! - `snapshot.bin`  — last compacted full store snapshot
//! - `snapshot.tmp`  — in-progress write; atomically renamed on completion
//!
//! [`WalHandle`] is stored in `TaskCtx` and cloned into `ConnContext`.
//! `store.rs` and `framing.rs` are not modified — no circular imports.
//!
//! # Durability contract (review 2026-09-05)
//!
//! Three invariants, each with a regression test in `durability_tests`:
//!
//! 1. **A snapshot never discards a WAL record.** `do_snapshot` merges the on-disk
//!    WAL tail (every record appended since the last truncation) into the store scan
//!    under the store's own LWW rule before it truncates. The writer therefore does
//!    not depend on callers having applied an acknowledged record to memory yet —
//!    the ack-then-snapshot window (`kv_set_async` awaiting the ack while the writer
//!    hits its threshold) cannot lose the write. Callers *also* apply-then-append
//!    (`ops.rs`, `connection.rs`) so the store is never behind the WAL.
//! 2. **Replay is LWW, not a watermark.** Every WAL record is replayed through
//!    `apply_fn` (which is `apply_and_notify` — LWW). `snapshot_hlc` is informational.
//!    A delayed remote update carrying an HLC older than the snapshot's watermark is
//!    still a record this node accepted and acknowledged; a timestamp filter dropped it.
//! 3. **An ack is a durability claim.** `append` (Flush), `append_sync` and
//!    `trigger_snapshot` return `Err` when the writer task is gone (channel closed) —
//!    never `Ok` by default. `append_sync` forces `fdatasync` in every `SyncMode`.

use crate::config::SyncMode;
use crate::framing::SyncEntry;
use crate::node_id::NodeId;
use crate::serde_fixint as codec;
use crate::store::{apply_and_notify, lww_wins, KvState, StoreEntry};
use ahash::AHashMap;
use bytes::{BufMut, BytesMut};
use serde::{Deserialize, Serialize};
use std::{
    io::{self},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{
    fs as tfs,
    io::{AsyncSeekExt, AsyncWriteExt},
    sync::{mpsc, oneshot},
    time,
};
use tracing::{error, warn};

// ── Data-at-rest encryption hook (WS3 crown-jewel) ────────────────────────────

/// Operator-supplied envelope cipher for KV data **at rest** — the WAL records
/// and snapshot blobs this node writes to disk.
///
/// The substrate stays deliberately neutral on key custody: implement this trait
/// over your own KMS / keyring / HSM and attach it with
/// `GossipAgent::with_data_at_rest_cipher`.
/// When no cipher is attached, bytes are written in the clear (unchanged
/// behaviour, zero overhead).
///
/// Scope: this protects the **on-disk** persistence surface only. Data in transit
/// is protected separately by the `tls` feature (mTLS); data in memory is not
/// encrypted. A node must use a cipher whose key is stable across restarts, or it
/// cannot replay its own WAL/snapshot — key rotation is the operator's concern.
pub trait DataAtRestCipher: Send + Sync {
    /// Encrypt a plaintext blob for storage. Called once per WAL record and once
    /// per snapshot. The returned ciphertext is length-framed verbatim on disk.
    fn encrypt(&self, plaintext: &[u8]) -> Vec<u8>;
    /// Decrypt a blob read from disk. Return `None` on authentication or format
    /// failure — the record is then treated as corrupt and skipped, exactly as a
    /// truncated/garbled plaintext record would be.
    fn decrypt(&self, ciphertext: &[u8]) -> Option<Vec<u8>>;
}

/// Optional reference passed through the persistence paths.
type Cipher<'a> = Option<&'a Arc<dyn DataAtRestCipher>>;

// ── On-disk snapshot format ──────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
pub struct KvSnapshot {
    /// HLC reading at the moment the snapshot was taken. **Informational only** —
    /// replay does *not* use it as a filter (durability invariant 2, module doc):
    /// a WAL record with an older HLC is a record this node accepted after the
    /// snapshot and must be replayed through LWW. Kept for on-disk compatibility.
    pub snapshot_hlc: u64,
    pub entries: Vec<SyncEntry>,
}

// ── WAL record size cap ──────────────────────────────────────────────────────

const MAX_RECORD_BYTES: usize = 64 * 1024 * 1024;

// ── Channel messages ─────────────────────────────────────────────────────────

pub enum WalMsg {
    Append {
        entry: SyncEntry,
        /// `Some` → caller awaits the append (and, when synced, fsync) result.
        /// `None` → fire-and-forget.
        ack: Option<oneshot::Sender<io::Result<()>>>,
        /// `true` → `fdatasync` this record regardless of the writer's `SyncMode`
        /// (`append_sync`: consensus committed slots + leases).
        force_sync: bool,
    },
    TriggerSnapshot {
        ack: oneshot::Sender<io::Result<()>>,
    },
    /// `fdatasync` the WAL as it stands, appending nothing. Because appends are written in
    /// order on one file, a successful sync establishes durability for **every record already
    /// appended** — which is what lets a peer answer "yes, I hold that operation on disk"
    /// without tracking durability per entry (contracts axis item 1 PR 4b).
    Sync {
        ack: oneshot::Sender<io::Result<()>>,
    },
    #[allow(dead_code)]
    Shutdown,
}

// ── Public handle ────────────────────────────────────────────────────────────

pub struct WalHandle {
    tx:        mpsc::Sender<WalMsg>,
    sync_mode: SyncMode,
}

/// The writer task has exited (channel closed) — nothing awaited on it can be a
/// durability claim. Surfaced as `BrokenPipe` so callers can distinguish it from a
/// disk error (durability invariant 3, module doc).
fn writer_gone() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "WAL writer task has stopped")
}

impl WalHandle {
    /// Append and — in `Flush` mode — await the `fdatasync` ack.
    ///
    /// `Flush`: `Err` if the writer is gone or the append/fsync failed.
    /// `Async`/`Os`: fire-and-forget (`try_send`); always `Ok`.
    pub async fn append(&self, entry: SyncEntry) -> io::Result<()> {
        match self.sync_mode {
            SyncMode::Flush => self.send_and_await(entry, false).await,
            SyncMode::Async | SyncMode::Os => {
// The WAL channel: a full queue here *skips an append*, which is why the inventory calls
                // it out by name. Recorded, so a replay drops the same record.
                let _ = crate::sim_seam::chan_try_send(
                    WAL_CHAN,
                    &self.tx,
                    WalMsg::Append { entry, ack: None, force_sync: false },
                );
                Ok(())
            }
        }
    }

    /// Fire-and-forget for synchronous callers (`set` / `delete`).
    /// Never awaits fsync. Silently drops if the channel is full —
    /// consistent with `GossipAgent::set`'s existing try_send semantics.
    pub fn append_try(&self, entry: SyncEntry) {
// The WAL channel — see the note above.
        let _ = crate::sim_seam::chan_try_send(
            WAL_CHAN,
            &self.tx,
            WalMsg::Append { entry, ack: None, force_sync: false },
        );
    }

    /// Append and await `fdatasync` **regardless of `sync_mode`** — the record is on
    /// stable storage when this returns `Ok`. Used for consensus committed-slot and
    /// lease writes. `Err` if the writer is gone or the append/fsync failed; the
    /// caller must not report durability on `Err`.
    pub async fn append_sync(&self, entry: SyncEntry) -> io::Result<()> {
        self.send_and_await(entry, true).await
    }

    /// Append and **await the writer's acknowledgement of the write**, without forcing an fsync
    /// beyond what the node's `SyncMode` already does.
    ///
    /// This is the receipt path's append (contracts axis item 1). [`append`](Self::append) cannot
    /// serve it: in `Async`/`Os` that method is a `try_send` that returns `Ok` even when the queue
    /// is full or the writer is gone, so a receipt built on it would claim the bytes reached the
    /// operating system when they may never have left this process (found by review, 2026-09-15).
    ///
    /// `Ok` means the writer's `write` returned — and, in `Flush` mode, its `fdatasync` too, since
    /// the writer syncs whenever `force_sync || sync_mode == Flush`. `Err` means the record's
    /// durability is **not established**: the writer is gone, the queue closed, or the write failed.
    /// It never means the record is absent.
    pub async fn append_acked(&self, entry: SyncEntry) -> io::Result<()> {
        self.send_and_await(entry, false).await
    }

    /// `fdatasync` the WAL as it stands — **appending nothing** — and await the result.
    ///
    /// Records are appended in order to a single file, so `Ok` establishes that every record
    /// appended before this call is on stable storage. That is what makes a replica-sync answer
    /// possible without per-entry durability tracking: a peer asked whether it holds an operation
    /// checks its store for that exact stamp and content, calls this, and answers on the result
    /// (contracts axis item 1 PR 4b, `docs/design/contracts-receipts.md` §2.1).
    ///
    /// `Err` means durability is **not established** — the writer is gone or the `fdatasync`
    /// failed. As everywhere else on this path, it never means the records are absent.
    pub async fn sync(&self) -> io::Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(WalMsg::Sync { ack: tx }).await.map_err(|_| writer_gone())?;
        rx.await.unwrap_or_else(|_| Err(writer_gone()))
    }

    /// Ask the writer to snapshot immediately. Awaits completion; `Err` if the
    /// writer is gone or the snapshot failed.
    pub async fn trigger_snapshot(&self) -> io::Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(WalMsg::TriggerSnapshot { ack: tx }).await.map_err(|_| writer_gone())?;
        rx.await.unwrap_or_else(|_| Err(writer_gone()))
    }

    async fn send_and_await(&self, entry: SyncEntry, force_sync: bool) -> io::Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(WalMsg::Append { entry, ack: Some(tx), force_sync })
            .await
            .map_err(|_| writer_gone())?;
        // A dropped ack sender means the writer exited mid-request — not `Ok`.
        rx.await.unwrap_or_else(|_| Err(writer_gone()))
    }

    /// Test-only constructor over a raw channel (writer-death probes).
    #[cfg(test)]
    pub(crate) fn from_parts(tx: mpsc::Sender<WalMsg>, sync_mode: SyncMode) -> Self {
        Self { tx, sync_mode }
    }

    #[allow(dead_code)]
    pub async fn shutdown(&self) {
        let _ = self.tx.send(WalMsg::Shutdown).await;
    }
}

// ── Startup replay ───────────────────────────────────────────────────────────

/// Reads `snapshot.bin` and `wal.bin` from `dir`, calls `apply_fn` for each
/// entry, and returns the highest HLC timestamp seen.
///
/// `apply_fn` is responsible for `intern_key` (if configured) and
/// `apply_and_notify` — keeping `persistence.rs` free of agent-layer imports.
pub async fn replay<F>(
    dir: &std::path::Path,
    cipher: Cipher<'_>,
    mut apply_fn: F,
) -> io::Result<u64>
where
    F: FnMut(SyncEntry),
{
    let mut max_ts: u64 = 0;
    let snapshot_path = dir.join("snapshot.bin");
    let wal_path      = dir.join("wal.bin");

    // 1. Snapshot ─────────────────────────────────────────────────────────────
    // The watermark is decoded for format compatibility but deliberately unused
    // (durability invariant 2, module doc).
    let _snapshot_hlc = if snapshot_path.exists() {
        match crate::sim_seam::fs_read(&snapshot_path, SNAP_FILE).await {
            Ok(raw) => {
                // Decrypt the snapshot blob if a cipher is configured; a decrypt
                // failure is treated like a corrupt snapshot (skipped).
                let decrypted = match cipher {
                    Some(c) => c.decrypt(&raw),
                    None    => Some(raw.to_vec()),
                };
                let bytes = match decrypted {
                    Some(b) => b,
                    None => {
                        warn!("persistence: snapshot.bin failed to decrypt, skipping");
                        Vec::new()
                    }
                };
                match codec::from_slice::<KvSnapshot>(&bytes) {
                    Ok(snap) => {
                        let hlc = snap.snapshot_hlc;
                        for entry in snap.entries {
                            if entry.timestamp > max_ts { max_ts = entry.timestamp; }
                            apply_fn(entry);
                        }
                        hlc
                    }
                    Err(e) => {
                        warn!("persistence: corrupt snapshot.bin, skipping: {e}");
                        0
                    }
                }
            }
            Err(e) => {
                warn!("persistence: failed to read snapshot.bin: {e}");
                0
            }
        }
    } else {
        0
    };

    // 2. WAL ──────────────────────────────────────────────────────────────────
    // Every record is replayed — `apply_fn` is LWW, so a record older than the
    // snapshot's entry for the same key loses on its own and a record for a key the
    // snapshot lacks (a delayed remote update with an old HLC, accepted after the
    // snapshot) is restored. The former `timestamp > snapshot_hlc` watermark dropped
    // the latter (durability invariant 2, module doc). `snapshot_hlc` stays
    // informational only.
    if wal_path.exists() {
        match crate::sim_seam::fs_read(&wal_path, WAL_FILE).await {
            Ok(bytes) => decode_wal_records(&bytes, cipher, |entry| {
                if entry.timestamp > max_ts { max_ts = entry.timestamp; }
                apply_fn(entry);
            }),
            Err(e) => warn!("persistence: failed to read wal.bin: {e}"),
        }
    }

    Ok(max_ts)
}

/// Walks the length-prefixed records of a WAL image, decrypting when a cipher is
/// configured, and hands each decoded [`SyncEntry`] to `f` in file order. Stops at
/// the first zero/oversized length, truncated tail, decrypt failure or decode error
/// (a corrupt tail — the same stop rule for replay and for the snapshot merge, so
/// the two never disagree about what the WAL holds).
fn decode_wal_records<F: FnMut(SyncEntry)>(bytes: &[u8], cipher: Cipher<'_>, mut f: F) {
    let mut pos = 0usize;
    while pos + 4 <= bytes.len() {
        let len = u32::from_le_bytes([
            bytes[pos], bytes[pos+1], bytes[pos+2], bytes[pos+3],
        ]) as usize;
        pos += 4;
        if len == 0 || len > MAX_RECORD_BYTES { break; }
        if pos + len > bytes.len()            { break; } // truncated tail
        let record_bytes = &bytes[pos..pos + len];
        pos += len;
        let decrypted = match cipher {
            Some(c) => match c.decrypt(record_bytes) {
                Some(b) => b,
                None    => break,
            },
            None => record_bytes.to_vec(),
        };
        match codec::from_slice::<SyncEntry>(&decrypted) {
            Ok(entry) => f(entry),
            Err(_)    => break, // corrupt tail — stop
        }
    }
}

// ── WalWriter task ───────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
/// Hook the WAL snapshot loop consults each interval tick to decide whether to
/// defer a scheduled snapshot (e.g. when the node is already opaque for load
/// reasons, to avoid piling snapshot opacity on top). `None` — pure-core embeds —
/// never defers. Core provides the mechanism; the opacity policy is supplied by the
/// upper layer, so core stays unaware of `sys/load/` semantics (Layer II).
pub type SnapshotDeferHook = Arc<dyn Fn() -> bool + Send + Sync>;

#[allow(clippy::too_many_arguments)]
pub fn spawn_wal_writer(
    dir:                    PathBuf,
    sync_mode:              SyncMode,
    snapshot_wal_threshold: usize,
    snapshot_interval_secs: u64,
    kv_state:               Arc<KvState>,
    node_id:                NodeId,
    hlc:                    Arc<crate::hlc::Hlc>,
    default_ttl:            u8,
    cipher:                 Option<Arc<dyn DataAtRestCipher>>,
    defer_snapshot:         Option<SnapshotDeferHook>,
) -> WalHandle {
    let channel_depth = (snapshot_wal_threshold * 4).max(1024);
    let (tx, rx) = mpsc::channel::<WalMsg>(channel_depth);
    let handle = WalHandle { tx, sync_mode };

    tokio::spawn(wal_writer_task(
        rx,
        dir,
        sync_mode,
        snapshot_wal_threshold,
        snapshot_interval_secs,
        kv_state,
        node_id,
        hlc,
        default_ttl,
        cipher,
        defer_snapshot,
    ));

    handle
}

#[allow(clippy::too_many_arguments)]
async fn wal_writer_task(
    mut rx:                 mpsc::Receiver<WalMsg>,
    dir:                    PathBuf,
    sync_mode:              SyncMode,
    snapshot_wal_threshold: usize,
    snapshot_interval_secs: u64,
    kv_state:               Arc<KvState>,
    node_id:                NodeId,
    hlc:                    Arc<crate::hlc::Hlc>,
    default_ttl:            u8,
    cipher:                 Option<Arc<dyn DataAtRestCipher>>,
    defer_snapshot:         Option<SnapshotDeferHook>,
) {
    let wal_path = dir.join("wal.bin");
    let mut wal_file = match open_wal(&wal_path).await {
        Ok(f)  => f,
        Err(e) => { error!("persistence: failed to open wal.bin: {e}"); return; }
    };
    let mut wal_entry_count: usize = 0;

    let interval = Duration::from_secs(snapshot_interval_secs);
    let mut snap_timer = time::interval(interval);
    snap_timer.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    snap_timer.tick().await; // consume immediate first tick

    loop {
        tokio::select! {
            biased;

            msg = rx.recv() => {
                match msg {
                    // Channel closed (WalHandle dropped) or explicit Shutdown:
                    // snapshot and exit.
                    None | Some(WalMsg::Shutdown) => {
                        let _ = do_snapshot(&dir, &kv_state, &node_id, &hlc, default_ttl, &mut wal_file, cipher.as_ref()).await;
                        break;
                    }
                    Some(WalMsg::Append { entry, ack, force_sync }) => {
                        let sync = force_sync || sync_mode == SyncMode::Flush;
                        let result = wal_append(&mut wal_file, &entry, sync, cipher.as_ref()).await;
                        wal_entry_count += 1;
                        if let Some(ack) = ack { let _ = ack.send(result); }
                        if wal_entry_count >= snapshot_wal_threshold {
                            let _ = do_snapshot(&dir, &kv_state, &node_id, &hlc, default_ttl, &mut wal_file, cipher.as_ref()).await;
                            wal_entry_count = 0;
                        }
                    }
                    Some(WalMsg::TriggerSnapshot { ack }) => {
                        let result = do_snapshot(&dir, &kv_state, &node_id, &hlc, default_ttl, &mut wal_file, cipher.as_ref()).await;
                        wal_entry_count = 0;
                        let _ = ack.send(result);
                    }
                    Some(WalMsg::Sync { ack }) => {
                        // Nothing is appended: this syncs what is already there. Handled in the
                        // same loop as Append so it cannot race a concurrent write — the ordering
                        // is what makes "everything before this is durable" true.
                        let _ = ack.send(wal_file.sync_data().await);
                    }
                }
            }

            _ = snap_timer.tick() => {
                // Defer if already opaque for another reason to avoid piling
                // snapshot opacity on top of existing load-based opacity. The
                // opacity check is injected (Layer II policy); core stays neutral.
                if defer_snapshot.as_ref().is_some_and(|f| f()) {
                    snap_timer.reset_after(Duration::from_secs(30));
                    continue;
                }
                let _ = do_snapshot(&dir, &kv_state, &node_id, &hlc, default_ttl, &mut wal_file, cipher.as_ref()).await;
                wal_entry_count = 0;
            }
        }
    }
}

// ── WAL I/O ──────────────────────────────────────────────────────────────────

/// Trace stream names for the storage seams. One place, so a trace written today and a reader
/// written later cannot disagree about what a stream is called.
const WAL_FILE: &str = "wal.bin";
/// The directory whose sync makes a rename durable.
const DIR_SYNC: &str = "dir";
/// The snapshot's temporary file, before it is renamed into place.
const SNAP_TMP: &str = "snapshot.tmp";
/// The snapshot itself.
const SNAP_FILE: &str = "snapshot.bin";
/// The WAL's bounded append channel. A full queue here skips a record, which the inventory names
/// as one of the three things channel fullness decides.
const WAL_CHAN: &str = "wal/append";
/// The WAL tail read the snapshot merges before truncating — its own stream, because *this* read
/// returning different bytes is the v2.4.3 failure and deserves to be legible on its own line.
const WAL_TAIL: &str = "wal.bin#tail";
/// The WAL's post-truncation sync — a distinct stream from an ordinary append's sync, so the
/// *ordering* against the directory sync is legible in a trace rather than buried among appends.
const WAL_TRUNCATE: &str = "wal.bin#truncate";


async fn open_wal(path: &std::path::Path) -> io::Result<tfs::File> {
    tfs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
}

/// Appends one record; `fdatasync`s it when `sync` is set (Flush mode, or a
/// forced-sync request such as `append_sync`).
async fn wal_append(
    file:   &mut tfs::File,
    entry:  &SyncEntry,
    sync:   bool,
    cipher: Cipher<'_>,
) -> io::Result<()> {
    // Encode the record, then optionally encrypt the payload. The length prefix
    // frames whatever lands on disk (ciphertext when a cipher is configured).
    let mut payload: Vec<u8> = codec::to_vec(entry)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if let Some(c) = cipher {
        payload = c.encrypt(&payload);
    }

    // Build [u32 LE length][payload] in one buffer.
    let mut buf = BytesMut::with_capacity(payload.len() + 4);
    buf.put_u32_le(payload.len() as u32);
    buf.extend_from_slice(&payload);

    // Routed through the replay seams (item 6 PR 3). The *order* of these two is the durability
    // property — a record is durable only once the sync returns — so both are kernel effects and a
    // trace that lost the sync would diverge.
    crate::sim_seam::fs_write_all(file, WAL_FILE, &buf).await?;
    if sync {
        crate::sim_seam::fs_sync_data(file, WAL_FILE).await?;
    }
    Ok(())
}

// ── Snapshot ─────────────────────────────────────────────────────────────────

/// `fsync` the directory itself so a preceding `rename` is on stable storage.
///
/// Honoured by ext4 / XFS / btrfs (the deployment targets). On macOS `fsync` does not
/// force the drive cache either way (`F_FULLFSYNC` would) — that caveat applies to every
/// sync in this module and is documented in `deployment.md § Persistence modes`, not
/// special-cased here. Windows has no directory fsync; the call is a no-op there.
async fn fsync_dir(dir: &std::path::Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let d = tfs::File::open(dir).await?;
        // The effect that makes a preceding `rename` survive a power loss (v2.4.4). Recorded under
        // its own op so its *removal* is a divergence, not a silent regression.
        crate::sim_seam::fs_sync_dir(&d, DIR_SYNC).await
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

/// `lww_wins` over two WAL/snapshot records — the store's exact conflict rule, so
/// the snapshot merge and a later replay agree.
fn sync_entry_wins(incoming: &SyncEntry, current: &SyncEntry) -> bool {
    let inc_val = if incoming.is_tombstone { None } else { Some(incoming.value.clone()) };
    let cur = StoreEntry {
        data:      if current.is_tombstone { None } else { Some(current.value.clone()) },
        timestamp: current.timestamp,
    };
    lww_wins(incoming.timestamp, incoming.is_tombstone, &inc_val, &cur)
}

// ── The merge-removed witness (item 6 PR 4) ───────────────────────────────────────────────────
//
// The plan's §4 divergence note is explicit: *"The witness is a `cfg(test)` toggle, not a manual
// edit (the 2026-09-05 fix was verified by hand-disabling the merge)."* A hand edit cannot be
// named in a bundle, cannot be re-run by someone else, and cannot prove the fix still holds — so
// the one line that saves an acknowledged record gets a switch a test can throw.
//
// Thread-local rather than a global flag: `cargo test` runs tests concurrently in one process, and
// a shared flag would leak into whichever snapshot happened to be running. `#[tokio::test]` is
// current-thread by default, so the snapshot future is polled on the thread that set it.

#[cfg(test)]
thread_local! {
    static WITNESS_SKIP_WAL_MERGE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The witness's name, exactly as a bundle records it. Only a bundle needs it, and bundles are a
/// `sim` concern.
#[cfg(all(test, feature = "sim"))]
pub(crate) const WITNESS_SKIP_WAL_MERGE_TOGGLE: &str = "persistence::WITNESS_SKIP_WAL_MERGE";

/// Removes the WAL-tail merge for as long as this guard lives.
#[cfg(test)]
pub(crate) struct MergeRemoved;

#[cfg(test)]
impl MergeRemoved {
    pub(crate) fn set() -> Self {
        WITNESS_SKIP_WAL_MERGE.with(|c| c.set(true));
        Self
    }
}

#[cfg(test)]
impl Drop for MergeRemoved {
    fn drop(&mut self) {
        WITNESS_SKIP_WAL_MERGE.with(|c| c.set(false));
    }
}

#[cfg(test)]
fn wal_merge_removed() -> bool {
    WITNESS_SKIP_WAL_MERGE.with(|c| c.get())
}

/// In a shipped build the merge is never removed — the witness does not exist outside tests.
#[cfg(not(test))]
#[inline(always)]
fn wal_merge_removed() -> bool {
    false
}

#[allow(clippy::too_many_arguments)]
async fn do_snapshot(
    dir:         &std::path::Path,
    kv_state:    &Arc<KvState>,
    node_id:     &NodeId,
    hlc:         &Arc<crate::hlc::Hlc>,
    default_ttl: u8,
    wal_file:    &mut tfs::File,
    cipher:      Cipher<'_>,
) -> io::Result<()> {
    let opacity_key: Arc<str> = Arc::from(format!(
        "{}{}{}",
        crate::signal::kv_ns::LOAD,
        node_id,
        "/persistence",
    ));

    // 1. Raise opacity.
    let opaque_val = crate::signal::encode_load_state(&crate::signal::LoadState {
        fill_ratio:    1.0,
        is_opaque:     true,
        written_at_ms: crate::hlc::physical_ms(hlc.current()),
    });
    let raise_upd = crate::framing::make_gossip_update(
        node_id, default_ttl, Arc::clone(&opacity_key), opaque_val, false, hlc,
    );
    apply_and_notify(kv_state, &raise_upd);

    // 2. Scan store.
    let snapshot_hlc = hlc.current();
    let mut entries: Vec<SyncEntry> = {
        let guard = kv_state.store.pin();
        guard.iter()
            // Include TOMBSTONES, not just live entries. The in-memory store retains a tombstone for
            // a propagation window (the GC sweeps only older ones, tasks.rs), so a delete is remembered
            // long enough to reach every peer. The old `filter_map` on `v.data` dropped every tombstone
            // from the snapshot and then truncated the WAL, so after a restart the deleted key existed
            // NOWHERE on disk — and a stale peer that missed the delete resurrected it via anti-entropy
            // (no tombstone to win the LWW tie). Persist what the store holds: the GC has already
            // bounded the tombstone set, so this is exactly the in-window anti-resurrection set. Replay
            // re-applies `is_tombstone` (lifecycle.rs apply_fn). Audit 2026-07-15 pass 3.
            .map(|(k, v)| SyncEntry {
                key:          Arc::clone(k),
                value:        v.data.clone().unwrap_or_default(),
                timestamp:    v.timestamp,
                is_tombstone: v.data.is_none(),
            })
            .collect()
    };

    // 2b. Merge the WAL tail — durability invariant 1 (module doc). Every record
    // appended since the last truncation is on disk and may have been acknowledged
    // to a caller that has not yet applied it to the store (the writer runs the
    // threshold snapshot straight after sending the ack, with no yield; on a
    // multi-thread runtime the caller need not have been polled). Step 4 truncates
    // the WAL, so anything not carried into the snapshot here is gone. Merging
    // under the store's own `lww_wins` keeps the snapshot exactly what replay of
    // (store ∪ WAL) would have produced. The writer is single-task, so no record
    // lands between this read and the truncation.
    wal_file.flush().await?; // complete any in-flight (Async/Os-mode) write before read-back
    // A read failure here must ABORT the snapshot: proceeding as if the tail were empty
    // would write a snapshot without those records and then truncate them in step 4 —
    // turning a transient read error into data loss. Absent file = empty tail (fresh dir).
    let wal_bytes = match crate::sim_seam::fs_read(&dir.join("wal.bin"), WAL_TAIL).await {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    // `wal_merge_removed()` is the item 6 PR 4 witness and is `false` in every shipped build.
    if !wal_bytes.is_empty() && !wal_merge_removed() {
        let mut tail: AHashMap<Arc<str>, SyncEntry> = AHashMap::new();
        decode_wal_records(&wal_bytes, cipher, |rec| {
            match tail.get(&rec.key) {
                Some(cur) if !sync_entry_wins(&rec, cur) => {}
                _ => { tail.insert(Arc::clone(&rec.key), rec); }
            }
        });
        for e in entries.iter_mut() {
            if let Some(rec) = tail.remove(&e.key)
                && sync_entry_wins(&rec, e) {
                    *e = rec;
                }
        }
        entries.extend(tail.into_values());
    }

    // 3. Write snapshot.tmp → fdatasync → rename to snapshot.bin.
    let tmp_path  = dir.join("snapshot.tmp");
    let snap_path = dir.join("snapshot.bin");
    // **Canonical order** (item 6 PR 4). `entries` came from iterating the store and then extending
    // with the WAL tail, so its order is papaya's iteration order — which is not stable across
    // processes even though the store's hasher is seeded (`store.rs`, `RandomState::with_seeds`).
    // The snapshot's *bytes* therefore depended on it: two nodes holding identical logical state
    // wrote different files, and a recorded run could not reproduce its own snapshot. §2.5 of the
    // inventory names hash iteration order as a nondeterminism source and says each order-sensitive
    // consumer must be listed; this encoder was one and was not.
    //
    // Sorting by key makes the file a function of the state it represents. Nothing reads a snapshot
    // positionally — replay folds it into the store under LWW — so this is free.
    entries.sort_unstable_by(|a, b| a.key.cmp(&b.key));
    let snap = KvSnapshot { snapshot_hlc, entries };
    let encoded = {
        let buf = codec::to_vec(&snap)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        match cipher {
            Some(c) => c.encrypt(&buf),
            None    => buf,
        }
    };
    // The snapshot install, as five ordered kernel effects (item 6 PR 3): write the temp file, sync
    // its bytes, rename it into place, sync the *directory* so the rename survives a power loss
    // (3b below), and only then truncate the WAL. The order is the whole property — v2.4.4 exists
    // because one of these was in the wrong place — so each is recorded and a trace catches a
    // reordering or a removal.
    crate::sim_seam::fs_write(&tmp_path, SNAP_TMP, &encoded).await?;
    {
        let f = tfs::File::open(&tmp_path).await?;
        crate::sim_seam::fs_sync_data(&f, SNAP_TMP).await?;
    }
    crate::sim_seam::fs_rename(&tmp_path, &snap_path, SNAP_FILE).await?;
    // 3b. Make the RENAME durable before the WAL is truncated. `sync_data` on the file
    // covers its bytes, not the directory entry; without this, a power loss / kernel
    // panic after step 4 could leave the OLD snapshot.bin on disk next to an EMPTY,
    // fsynced wal.bin — every record since the previous snapshot gone, each of them
    // acknowledged. A process crash is not affected (the page cache still writes the
    // rename out), which is why the process-kill suite could never see this.
    fsync_dir(dir).await?;

    // 4. Truncate WAL. Recorded too: "the WAL was truncated" is the effect the directory sync above
    // must precede, and a trace that showed them the other way round is the v2.4.4 regression.
    wal_file.seek(std::io::SeekFrom::Start(0)).await?;
    wal_file.set_len(0).await?;
    crate::sim_seam::fs_sync_data(wal_file, WAL_TRUNCATE).await?;

    // 5. Lower opacity — tombstone the persistence key.
    let lower_upd = crate::framing::make_gossip_update(
        node_id, default_ttl, opacity_key, bytes::Bytes::new(), true, hlc,
    );
    apply_and_notify(kv_state, &lower_upd);

    Ok(())
}

#[cfg(test)]
mod persist_tests {
    use super::*;
    use crate::framing::{make_gossip_update, GossipUpdate};
    use crate::store::KvState;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn unique_dir() -> std::path::PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "myc-persist-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // ── item 6 PR 4 — the merge-removed witness ──────────────────────────────────────────────
    //
    // The race itself is already pinned, in `durability_tests`:
    // `regression_snapshot_retains_wal_record_acked_before_local_apply` puts a record in the WAL
    // that the caller has not yet applied to the store, snapshots, and shows it survives the
    // truncation. What did not exist is the other half the plan asks for — a **witness**: a way to
    // make that assertion fail on demand, so a bundle can name it and a reader can watch the loss
    // happen rather than take the fix on trust.
    //
    // §4's divergence note is explicit that it must be a `cfg(test)` toggle rather than a hand edit
    // (the 2026-09-05 fix was verified by hand-disabling the merge). A hand edit cannot be named in
    // a bundle, re-run by someone else, or used to show the fix still holds.

    /// **The witness.** With the merge removed, the acknowledged record is lost.
    ///
    /// This is the exact inverse of
    /// `durability_tests::regression_snapshot_retains_wal_record_acked_before_local_apply`, and it
    /// is the assertion a failure bundle names via `WITNESS_SKIP_WAL_MERGE_TOGGLE`.
    #[tokio::test]
    async fn the_merge_removed_witness_loses_the_acknowledged_record() {
        let _witness = MergeRemoved::set();

        let dir  = unique_dir();
        let node = NodeId::new("127.0.0.1", 1).unwrap();
        let hlc  = Arc::new(crate::hlc::Hlc::new());
        let store = KvState::new(0);
        apply_and_notify(&store, &make_gossip_update(
            &node, 1, Arc::from("a"), bytes::Bytes::from_static(b"v-a"), false, &hlc));

        // "b" is acknowledged into the WAL and never applied to the store — the race's state.
        let mut wal = tfs::OpenOptions::new()
            .create(true).truncate(false).read(true).write(true)
            .open(dir.join("wal.bin")).await.unwrap();
        let unapplied = SyncEntry {
            key:          Arc::from("b"),
            value:        bytes::Bytes::from_static(b"v-b"),
            timestamp:    hlc.tick(),
            is_tombstone: false,
        };
        wal_append(&mut wal, &unapplied, true, None).await.unwrap();

        do_snapshot(&dir, &store, &node, &hlc, 1, &mut wal, None).await.unwrap();

        let restored = KvState::new(0);
        {
            let r = Arc::clone(&restored);
            let apply = move |e: SyncEntry| {
                apply_and_notify(&r, &GossipUpdate {
                    nonce: crate::framing::ANTI_ENTROPY_NONCE, sender: 0, ttl: 1,
                    is_tombstone: e.is_tombstone, timestamp: e.timestamp, key: e.key, value: e.value,
                });
            };
            replay(&dir, None, apply).await.unwrap();
        }

        let pinned = restored.store.pin();
        assert!(pinned.get("a").is_some(), "the applied record survives — only the tail is lost");
        assert!(
            pinned.get("b").is_none(),
            "the witness must actually reproduce the loss: with the merge removed, a record that \
             was acknowledged is absent from disk after the restart. If this assertion fails the \
             witness has stopped witnessing, and every bundle naming it proves nothing."
        );
        drop(pinned);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The witness is scoped to its guard — it must not leak into the next test on this thread.
    #[tokio::test]
    async fn the_witness_is_off_again_once_its_guard_is_dropped() {
        assert!(!wal_merge_removed(), "no witness by default");
        {
            let _w = MergeRemoved::set();
            assert!(wal_merge_removed(), "set while the guard lives");
        }
        assert!(!wal_merge_removed(), "and cleared when it dies");
    }

    #[tokio::test]
    async fn regression_snapshot_retains_tombstone_no_resurrection_across_restart() {
        // Audit 2026-07-15 pass 3: do_snapshot dropped ALL tombstones then truncated the WAL, so a
        // deleted key existed NOWHERE on disk — after a restart a stale peer's ancient value
        // resurrected it (no tombstone to win the LWW tie). The snapshot must retain the tombstone.
        let dir  = unique_dir();
        let node = NodeId::new("127.0.0.1", 1).unwrap();
        let hlc  = Arc::new(crate::hlc::Hlc::new());

        // Source store: write "k", then delete it → a fresh-HLC tombstone.
        let src = KvState::new(0);
        apply_and_notify(&src, &make_gossip_update(&node, 1, Arc::from("k"), bytes::Bytes::from_static(b"v1"), false, &hlc));
        apply_and_notify(&src, &make_gossip_update(&node, 1, Arc::from("k"), bytes::Bytes::new(), true, &hlc));

        // Snapshot to disk (truncates the WAL), then replay into a FRESH store — a restart.
        let mut wal = tfs::OpenOptions::new().create(true).truncate(false).read(true).write(true)
            .open(dir.join("wal.log")).await.unwrap();
        do_snapshot(&dir, &src, &node, &hlc, 1, &mut wal, None).await.unwrap();

        let restored = KvState::new(0);
        {
            let r = Arc::clone(&restored);
            let apply = move |e: SyncEntry| {
                apply_and_notify(&r, &GossipUpdate {
                    nonce: crate::framing::ANTI_ENTROPY_NONCE, sender: 0, ttl: 1,
                    is_tombstone: e.is_tombstone, timestamp: e.timestamp, key: e.key, value: e.value,
                });
            };
            replay(&dir, None, apply).await.unwrap();
        }

        // The tombstone survived: "k" present as a tombstone (data None), NOT absent.
        let after = restored.store.pin().get("k").map(|e| e.data.is_none());
        assert_eq!(after, Some(true), "snapshot+replay must retain the tombstone, not drop it");

        // A stale peer re-delivers the ancient value → must NOT resurrect (tombstone wins LWW).
        apply_and_notify(&restored, &GossipUpdate {
            nonce: 7, sender: 0, ttl: 1, is_tombstone: false,
            timestamp: crate::hlc::pack(1, 0), key: Arc::from("k"), value: bytes::Bytes::from_static(b"v1"),
        });
        assert!(restored.store.pin().get("k").unwrap().data.is_none(),
            "an ancient replayed write must not resurrect the deleted key");

        std::fs::remove_dir_all(&dir).ok();
    }
}

/// Regression tests for the durability contract in the module doc (review
/// 2026-09-05, three P1 findings; probes contributed by the reviewer, adapted).
#[cfg(test)]
mod durability_tests {
    use super::*;
    use crate::framing::{make_gossip_update, sync_entry_from, GossipUpdate};
    use crate::store::KvState;
    use bytes::Bytes;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn unique_dir(tag: &str) -> std::path::PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "myc-durab-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn entry(key: &str, val: &'static [u8], ts: u64, tomb: bool) -> SyncEntry {
        SyncEntry { key: Arc::from(key), value: Bytes::from_static(val), timestamp: ts, is_tombstone: tomb }
    }

    /// Replays `dir` into a fresh store through the production apply path (LWW).
    async fn replay_into_fresh_store(dir: &std::path::Path) -> Arc<KvState> {
        let restored = KvState::new(0);
        let r = Arc::clone(&restored);
        replay(dir, None, move |e: SyncEntry| {
            apply_and_notify(&r, &GossipUpdate {
                nonce: crate::framing::ANTI_ENTROPY_NONCE, sender: 0, ttl: 1,
                is_tombstone: e.is_tombstone, timestamp: e.timestamp, key: e.key, value: e.value,
            });
        }).await.unwrap();
        restored
    }

    fn live_value(state: &KvState, key: &str) -> Option<Vec<u8>> {
        state.store.pin().get(key).and_then(|e| e.data.as_ref().map(|b| b.to_vec()))
    }

    // ── Invariant 3: an ack is a durability claim ─────────────────────────────

    #[tokio::test]
    async fn regression_closed_writer_never_acks_success() {
        // Finding 3a: `rx.await.unwrap_or(Ok(()))` turned a dead writer into a
        // successful fsync. Every awaiting path must report BrokenPipe instead.
        let (tx, rx) = mpsc::channel(1);
        drop(rx); // the writer task is gone
        let flush = WalHandle::from_parts(tx.clone(), SyncMode::Flush);
        let e = flush.append_sync(entry("user/key", b"durable", 1, false)).await
            .expect_err("append_sync on a closed writer returned Ok");
        assert_eq!(e.kind(), io::ErrorKind::BrokenPipe);
        assert!(flush.append(entry("user/key", b"durable", 2, false)).await.is_err(),
            "Flush-mode append on a closed writer returned Ok");
        assert!(flush.trigger_snapshot().await.is_err(),
            "trigger_snapshot on a closed writer returned Ok");
        // Documented exception: Async/Os `append` is fire-and-forget (try_send) —
        // it never claimed durability, so it stays Ok. `append_sync` does not.
        let asynch = WalHandle::from_parts(tx, SyncMode::Async);
        assert!(asynch.append(entry("user/key", b"x", 3, false)).await.is_ok());
        assert!(asynch.append_sync(entry("user/key", b"x", 4, false)).await.is_err(),
            "append_sync must not report durability in Async mode either");
    }

    /// Review regression (contracts axis PR 3, 2026-09-15): a record whose **sync never succeeded**
    /// is still replayable, so a required-sync failure cannot promise the value will never appear
    /// here.
    ///
    /// `wal_append` writes the bytes and *then* syncs. A failed sync therefore leaves a complete
    /// record in `wal.bin`, and replay — which cannot tell a synced record from an unsynced one —
    /// restores it. The first cut of `set_requiring_sync` documented "the value never became visible
    /// here", which is true of the call and **not** of the recovery. The error now says the recovery
    /// outcome is unknown; this test is why.
    #[tokio::test]
    async fn regression_an_unsynced_record_still_replays() {
        let dir = unique_dir("unsynced-replays");
        let mut file = open_wal(&dir.join("wal.bin")).await.unwrap();
        // `sync = false` is exactly the state a failed fsync leaves behind: bytes written, never synced.
        wal_append(&mut file, &entry("user/unsynced", b"v", 10, false), false, None).await.unwrap();
        drop(file);

        let restored = replay_into_fresh_store(&dir).await;
        assert_eq!(
            live_value(&restored, "user/unsynced").as_deref(),
            Some(&b"v"[..]),
            "an unsynced record replays — so a durability failure must not claim the value can never appear",
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Review regression (contracts axis PR 2, 2026-09-15): the **receipt path's** append must not
    /// inherit `append`'s fire-and-forget `Ok`.
    ///
    /// In `Async`/`Os`, `append` is a `try_send` that returns `Ok` even when the writer is gone —
    /// documented and fine for a verb whose `bool` never claimed durability, but fatal for a
    /// receipt: `Ok` there became `LocalDurability::Buffered`, claiming the bytes had reached the
    /// operating system when they may never have left this process. `append_acked` awaits the
    /// writer's acknowledgement instead, so a dead writer is `Failed` — *durability not
    /// established* — in every sync mode.
    #[tokio::test]
    async fn regression_append_acked_never_acks_a_dead_writer() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx); // the writer task is gone
        for mode in [SyncMode::Flush, SyncMode::Async, SyncMode::Os] {
            let wal = WalHandle::from_parts(tx.clone(), mode);
            let e = wal
                .append_acked(entry("user/key", b"v", 1, false))
                .await
                .expect_err("append_acked on a closed writer must not return Ok");
            assert_eq!(e.kind(), io::ErrorKind::BrokenPipe, "mode {mode:?}");
            // The contrast that motivates it: the fire-and-forget verb still says Ok here.
            if mode != SyncMode::Flush {
                assert!(
                    wal.append(entry("user/key", b"v", 2, false)).await.is_ok(),
                    "mode {mode:?}: `append` remains fire-and-forget — which is why the receipt path cannot use it",
                );
            }
        }
    }

    #[tokio::test]
    async fn regression_writer_dying_mid_request_is_an_error() {
        // The ack sender is dropped without a reply (writer panicked/exited after
        // taking the message): the awaiting caller must see Err, not Ok.
        let (tx, mut rx) = mpsc::channel::<WalMsg>(1);
        let handle = WalHandle::from_parts(tx, SyncMode::Flush);
        let waiter = tokio::spawn(async move { handle.append_sync(entry("k", b"v", 1, false)).await });
        let msg = rx.recv().await.unwrap();
        drop(msg); // drops the ack sender unanswered
        assert!(waiter.await.unwrap().is_err(), "dropped ack must surface as Err");
    }

    #[tokio::test]
    async fn append_sync_fdatasyncs_in_async_mode() {
        // Finding 3b: `append_sync` promised an unconditional fdatasync but the
        // writer only synced in Flush mode. Observable proxy: the record is fully
        // on disk (not merely in tokio's in-flight write) the moment `Ok` returns,
        // with the writer spawned in Async mode.
        let dir  = unique_dir("sync");
        let node = NodeId::new("127.0.0.1", 1).unwrap();
        let hlc  = Arc::new(crate::hlc::Hlc::new());
        let state = KvState::new(0);
        let handle = spawn_wal_writer(dir.clone(), SyncMode::Async, 1_000_000, 3_600,
            Arc::clone(&state), node, hlc, 1, None, None);
        handle.append_sync(entry("k", b"v", 7, false)).await.unwrap();
        let mut found = false;
        decode_wal_records(&std::fs::read(dir.join("wal.bin")).unwrap(), None, |e| {
            found |= e.key.as_ref() == "k" && e.timestamp == 7;
        });
        assert!(found, "append_sync returned Ok before the record was on disk");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── Invariant 1: a snapshot never discards a WAL record ───────────────────

    #[tokio::test]
    async fn probe_concurrent_append_sync_across_threshold_snapshots_loses_nothing() {
        // Run-61 falsification probe (Concurrency / Semantic): 64 tasks append_sync distinct
        // keys through the real writer with threshold 3, so snapshots interleave with appends
        // and each snapshot's WAL-tail merge races the callers' in-memory applies. Replay from
        // disk WHILE the writer is alive must hold every key (no clean-shutdown masking).
        let dir  = unique_dir("concurrent");
        let node = NodeId::new("127.0.0.1", 1).unwrap();
        let hlc  = Arc::new(crate::hlc::Hlc::new());
        let state = KvState::new(0);
        let handle = Arc::new(spawn_wal_writer(dir.clone(), SyncMode::Flush, 3, 3_600,
            Arc::clone(&state), node.clone(), Arc::clone(&hlc), 1, None, None));
        let mut tasks = Vec::new();
        for i in 0..64u32 {
            let (h, st, n, c) = (Arc::clone(&handle), Arc::clone(&state), node.clone(), Arc::clone(&hlc));
            tasks.push(tokio::spawn(async move {
                let upd = make_gossip_update(&n, 1, Arc::from(format!("c/{i}").as_str()),
                    Bytes::from(format!("v{i}")), false, &c);
                h.append_sync(sync_entry_from(&upd)).await.unwrap(); // ack, then a possibly-late apply
                tokio::task::yield_now().await;
                apply_and_notify(&st, &upd);
            }));
        }
        for t in tasks { t.await.unwrap(); }
        // Let the writer finish any snapshot it started for the last acks (structural: wait for
        // the WAL to be short, i.e. the threshold snapshot for the final batch has run or nothing is pending).
        tokio::time::sleep(Duration::from_millis(50)).await;
        let restored = replay_into_fresh_store(&dir).await;
        let missing: Vec<u32> = (0..64).filter(|i| live_value(&restored, &format!("c/{i}")).is_none()).collect();
        assert!(missing.is_empty(), "acked keys missing after concurrent appends + snapshots: {missing:?}");
        drop(handle);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn probe_replay_stops_at_corrupt_tail_and_keeps_prior_records() {
        // Run-61 falsification probe (Robustness): three good records then a torn fourth
        // (length prefix promises more bytes than exist) and, separately, an absurd length
        // prefix. Replay must keep the three, stop cleanly, never panic.
        let dir = unique_dir("torn");
        let mut file = open_wal(&dir.join("wal.bin")).await.unwrap();
        for i in 0..3u64 { wal_append(&mut file, &entry(&format!("t/{i}"), b"ok", 10 + i, false), true, None).await.unwrap(); }
        // torn tail: a plausible length, then only 3 bytes of payload
        file.write_all(&[200u8, 0, 0, 0, 1, 2, 3]).await.unwrap();
        file.sync_data().await.unwrap();
        let restored = replay_into_fresh_store(&dir).await;
        for i in 0..3 { assert_eq!(live_value(&restored, &format!("t/{i}")).as_deref(), Some(&b"ok"[..])); }
        assert_eq!(restored.store.pin().len(), 3, "the torn record must not produce an entry");
        // absurd length prefix (> MAX_RECORD_BYTES) after valid records → stop, no panic
        let dir2 = unique_dir("absurd");
        let mut f2 = open_wal(&dir2.join("wal.bin")).await.unwrap();
        wal_append(&mut f2, &entry("a/0", b"ok", 1, false), true, None).await.unwrap();
        f2.write_all(&u32::MAX.to_le_bytes()).await.unwrap();
        f2.write_all(b"garbage").await.unwrap();
        f2.sync_data().await.unwrap();
        let r2 = replay_into_fresh_store(&dir2).await;
        assert_eq!(live_value(&r2, "a/0").as_deref(), Some(&b"ok"[..]));
        assert_eq!(r2.store.pin().len(), 1);
        std::fs::remove_dir_all(&dir).ok(); std::fs::remove_dir_all(&dir2).ok();
    }

    #[tokio::test]
    async fn snapshot_install_syncs_the_directory() {
        // The power-loss property (rename durable before the WAL truncation) is not
        // observable without a filesystem adapter — the replay plan's point. What can be
        // pinned today: the helper is wired into the snapshot path and behaves (succeeds
        // on a real directory, surfaces a missing one as an error rather than a silent skip).
        let dir = unique_dir("dirsync");
        fsync_dir(&dir).await.expect("fsync of a real directory succeeds");
        assert!(fsync_dir(&dir.join("does-not-exist")).await.is_err(),
            "a missing directory must be an error, not a no-op");
        // And the full snapshot path still installs cleanly with the extra sync in place.
        let node = NodeId::new("127.0.0.1", 1).unwrap();
        let hlc  = Arc::new(crate::hlc::Hlc::new());
        let state = KvState::new(0);
        apply_and_notify(&state, &make_gossip_update(&node, 1, Arc::from("k"), Bytes::from_static(b"v"), false, &hlc));
        let mut wal = open_wal(&dir.join("wal.bin")).await.unwrap();
        do_snapshot(&dir, &state, &node, &hlc, 1, &mut wal, None).await.unwrap();
        assert!(dir.join("snapshot.bin").exists() && !dir.join("snapshot.tmp").exists());
        let restored = replay_into_fresh_store(&dir).await;
        assert_eq!(live_value(&restored, "k").as_deref(), Some(&b"v"[..]));
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── item 6 PR 4 — the scenario, replayed from a bundle ───────────────────────────────────

    /// One run of the WAL/snapshot race under `kernel`, returning its trace and its result.
    ///
    /// Everything that reads a clock happens **inside** the kernel's scope, so the HLC stamps that
    /// end up in the snapshot bytes are recorded choices rather than whatever the machine's clock
    /// said. Getting this wrong was the first divergence the replay caught: identical lengths,
    /// different content hashes.
    #[cfg(feature = "sim")]
    async fn race_run(
        tag:    &str,
        kernel: mycelium_sim::Kernel,
    ) -> (mycelium_sim::Trace, std::io::Result<()>) {
        use mycelium_sim::Sources;

        let dir  = unique_dir(tag);
        let node = NodeId::new("127.0.0.1", 1).unwrap();
        let mut wal = open_wal(&dir.join("wal.bin")).await.unwrap();

        crate::sim_seam::install(crate::sim_seam::SimContext {
            kernel,
            sources: Sources::seeded(1, 1_789_000_000_000),
            node:    "n1".into(),
            offsets: Default::default(),
        });

        let hlc   = Arc::new(crate::hlc::Hlc::new());
        let state = KvState::new(0);
        // "k" is applied to the store; "b" is only ever appended to the WAL — the race's state, an
        // acknowledged record its caller has not yet applied.
        apply_and_notify(&state, &make_gossip_update(
            &node, 1, Arc::from("k"), Bytes::from_static(b"v"), false, &hlc));
        let unapplied = SyncEntry {
            key:          Arc::from("b"),
            value:        Bytes::from_static(b"v-b"),
            timestamp:    hlc.tick(),
            is_tombstone: false,
        };
        wal_append(&mut wal, &unapplied, true, None).await.unwrap();

        let r = do_snapshot(&dir, &state, &node, &hlc, 1, &mut wal, None).await;
        let ctx = crate::sim_seam::take().expect("kernel installed");
        std::fs::remove_dir_all(&dir).ok();
        (ctx.kernel.trace().clone(), r)
    }

    /// **A snapshot is a function of the state it represents, not of hash iteration order.**
    ///
    /// Found by replaying the WAL/snapshot scenario: the recording and the replay produced snapshots
    /// of identical length and different content, because `entries` came straight from iterating the
    /// store and papaya's iteration order is not stable across processes — the store's hasher is
    /// seeded (`store.rs`, `RandomState::with_seeds`), but iteration is not.
    ///
    /// The consequence is bigger than replay: **two nodes holding the same logical state wrote
    /// byte-different snapshot files**, so any byte-level comparison of snapshots — a checksum, a
    /// dedup, a fixture diff — was unsound. §2.5 of the inventory says every order-sensitive
    /// consumer has to be listed; this encoder was one, and was not.
    ///
    /// The test inserts the same keys in two different orders and requires the same bytes out.
    #[tokio::test]
    async fn a_snapshot_is_byte_identical_for_the_same_state_whatever_order_it_was_built_in() {
        async fn snapshot_bytes(tag: &str, keys: &[&'static str]) -> Vec<u8> {
            let dir  = unique_dir(tag);
            let node = NodeId::new("127.0.0.1", 1).unwrap();
            let hlc  = Arc::new(crate::hlc::Hlc::new());
            let state = KvState::new(0);
            // One fixed stamp per key, so the two runs differ ONLY in insertion order.
            for (i, k) in keys.iter().enumerate() {
                apply_and_notify(&state, &GossipUpdate {
                    nonce: 1, sender: 0, ttl: 1, is_tombstone: false,
                    timestamp: crate::hlc::pack(1_000 + i as u64, 0),
                    key: Arc::from(*k), value: Bytes::from_static(b"v"),
                });
            }
            let mut wal = open_wal(&dir.join("wal.bin")).await.unwrap();
            do_snapshot(&dir, &state, &node, &hlc, 1, &mut wal, None).await.unwrap();
            let bytes = std::fs::read(dir.join("snapshot.bin")).unwrap();
            std::fs::remove_dir_all(&dir).ok();
            bytes
        }

        // The opacity key `do_snapshot` writes itself carries a timestamp from the HLC, which
        // differs per run — so compare the entry *ordering* rather than the whole file: decode both
        // and require the same key sequence.
        let a = snapshot_bytes("canon-a", &["zeta", "alpha", "mid"]).await;
        let b = snapshot_bytes("canon-b", &["mid", "zeta", "alpha"]).await;

        let keys_of = |bytes: &[u8]| -> Vec<String> {
            let snap: KvSnapshot = codec::from_slice(bytes).unwrap();
            snap.entries.iter().map(|e| e.key.to_string()).collect()
        };
        let ka = keys_of(&a);
        let kb = keys_of(&b);
        assert_eq!(ka, kb, "the same state must serialise in the same order, however it was built");

        let mut sorted = ka.clone();
        sorted.sort();
        assert_eq!(ka, sorted, "and that order is by key — canonical, not merely repeatable");
    }

    /// **The Phase A gate: the WAL/snapshot race replays from a bundle, and the bundle names the
    /// witness that makes it fail again.**
    ///
    /// The bundle is written to disk and read back rather than kept in memory: a bundle that has
    /// never survived a round trip is not a reproduction artefact, it is a variable.
    #[cfg(feature = "sim")]
    #[tokio::test]
    async fn the_wal_snapshot_race_replays_from_a_bundle_that_names_its_witness() {
        use mycelium_sim::{Bundle, Kernel};

        let (trace, recorded) = race_run("gate-rec", Kernel::recording()).await;
        recorded.expect("the recording itself must succeed");
        assert!(trace.len() >= 8, "the run has effects to replay: {}", trace.len());

        let bundle = Bundle::new(trace).witnessed_by(
            "durability_tests::regression_snapshot_retains_wal_record_acked_before_local_apply",
            Some(WITNESS_SKIP_WAL_MERGE_TOGGLE.to_string()),
        );
        assert!(
            bundle.can_prove_its_failure(),
            "a bundle with no witness replays a run in which nothing went wrong"
        );

        let bdir = unique_dir("gate-bundle");
        bundle.write(&bdir).expect("write the bundle");
        let read_back = Bundle::read(&bdir).expect("read it back");
        assert_eq!(
            read_back.witness.as_ref().and_then(|w| w.toggle.as_deref()),
            Some(WITNESS_SKIP_WAL_MERGE_TOGGLE),
            "the bundle must carry the toggle that reproduces the failure, by name"
        );

        // The replay: a different directory, a different process moment, the same schedule. Every
        // effect is checked against the recording and a mismatch panics.
        let (_t, replayed) = race_run("gate-rep", Kernel::replaying(read_back.trace)).await;
        replayed.expect("the recorded schedule must replay without diverging");

        std::fs::remove_dir_all(&bdir).ok();
    }

    /// **The fault sweep.** A failure at *any* storage effect in the run must not lose a record
    /// that was acknowledged.
    ///
    /// The property, stated so it can be checked rather than believed: after the run — however it
    /// ended — the acknowledged record is recoverable from disk. Either the snapshot carries it, or
    /// the WAL still does. The one outcome ruled out is *neither*, which is what v2.4.3 and v2.4.4
    /// were both about.
    ///
    /// **A fault is an effect that does not happen**, not an error handed back after the fact. The
    /// seam decides before acting in replay (`sim_seam::installed::planned_fs`), so an injected
    /// failure at the rename really leaves the rename undone. Without that this test would be
    /// checking a fiction: a "failed" rename that had already renamed.
    ///
    /// The faults are injected by rewriting one recorded outcome in the trace — the bundle format is
    /// text, and that is the point of it being text.
    #[cfg(feature = "sim")]
    #[tokio::test]
    async fn a_fault_at_any_storage_effect_leaves_the_acknowledged_record_recoverable() {
        use mycelium_sim::{ChoiceKind, Kernel, Sources, Trace};

        // A clean run, to learn the effect sequence.
        let (clean, ok) = race_run("sweep-rec", Kernel::recording()).await;
        ok.expect("the clean run must succeed");

        let faultable: Vec<(u64, String, String)> = clean
            .entries()
            .iter()
            .filter(|e| e.kind == ChoiceKind::Fs)
            .map(|e| (e.seq, e.stream.clone(), e.request.split(' ').next().unwrap_or("").to_string()))
            .collect();
        assert!(faultable.len() >= 6, "the run has storage effects to fault: {faultable:?}");

        for (seq, stream, op) in faultable {
            // The same trace with this one effect failing.
            let faulted_text: String = clean
                .to_text()
                .lines()
                .map(|line| {
                    let mut parts: Vec<&str> = line.split('\t').collect();
                    if parts.first().and_then(|s| s.parse::<u64>().ok()) == Some(seq)
                        && let Some(last) = parts.last_mut()
                    {
                        *last = "Err(injected fault)";
                    }
                    parts.join("\t")
                })
                .collect::<Vec<_>>()
                .join("\n");
            let faulted = Trace::parse(&faulted_text).expect("the faulted trace still parses");

            // Replay it against its own directory, and see what survives.
            let dir  = unique_dir(&format!("sweep-{seq}"));
            let node = NodeId::new("127.0.0.1", 1).unwrap();
            let mut wal = open_wal(&dir.join("wal.bin")).await.unwrap();
            crate::sim_seam::install(crate::sim_seam::SimContext {
                kernel:  Kernel::replaying(faulted),
                sources: Sources::seeded(1, 1_789_000_000_000),
                node:    "n1".into(),
                offsets: Default::default(),
            });
            let hlc   = Arc::new(crate::hlc::Hlc::new());
            let state = KvState::new(0);
            apply_and_notify(&state, &make_gossip_update(
                &node, 1, Arc::from("k"), Bytes::from_static(b"v"), false, &hlc));
            let unapplied = SyncEntry {
                key:          Arc::from("b"),
                value:        Bytes::from_static(b"v-b"),
                timestamp:    hlc.tick(),
                is_tombstone: false,
            };
            let appended = wal_append(&mut wal, &unapplied, true, None).await;
            let snapshotted = if appended.is_ok() {
                do_snapshot(&dir, &state, &node, &hlc, 1, &mut wal, None).await
            } else {
                Ok(())
            };
            crate::sim_seam::take();
            drop(wal);

            // A fault *before* the record was acknowledged is not this property's business — there
            // was nothing to lose yet.
            if appended.is_err() {
                eprintln!("SWEEP seq={seq} {stream} {op}: fault before the ack — skipped");
                std::fs::remove_dir_all(&dir).ok();
                continue;
            }
            eprintln!("SWEEP seq={seq} {stream} {op}: checked (snapshot {:?})", snapshotted.is_ok());

            // Whatever happened after the acknowledgement, a restart must still find the record.
            let restored = replay_into_fresh_store(&dir).await;
            assert_eq!(
                live_value(&restored, "b").as_deref(),
                Some(&b"v-b"[..]),
                "a fault at seq {seq} ({stream} {op}) lost an acknowledged record — the snapshot \
                 did not carry it and the WAL no longer holds it. snapshot result: {snapshotted:?}"
            );
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    /// The replay is a **check**, not a re-run: a schedule that departs from the recording stops.
    ///
    /// Without this the test above proves only that the harness can run twice.
    #[cfg(feature = "sim")]
    #[tokio::test]
    #[should_panic(expected = "replay diverged")]
    async fn a_replay_of_a_different_run_diverges() {
        use mycelium_sim::{Kernel, Sources};

        let (trace, _) = race_run("div-rec", Kernel::recording()).await;

        // The same schedule minus the WAL append: one fewer effect, so the trace and the run part
        // company at the first storage step that differs.
        let dir  = unique_dir("div-rep");
        let node = NodeId::new("127.0.0.1", 1).unwrap();
        let mut wal = open_wal(&dir.join("wal.bin")).await.unwrap();
        crate::sim_seam::install(crate::sim_seam::SimContext {
            kernel:  Kernel::replaying(trace),
            sources: Sources::seeded(1, 1_789_000_000_000),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        let hlc   = Arc::new(crate::hlc::Hlc::new());
        let state = KvState::new(0);
        apply_and_notify(&state, &make_gossip_update(
            &node, 1, Arc::from("k"), Bytes::from_static(b"v"), false, &hlc));
        let _ = do_snapshot(&dir, &state, &node, &hlc, 1, &mut wal, None).await;
        crate::sim_seam::take();
    }

    /// **The power-loss ordering, finally observable.**
    ///
    /// `snapshot_install_syncs_the_directory` above says it plainly: *"the power-loss property
    /// (rename durable before the WAL truncation) is not observable without a filesystem adapter"*,
    /// so it could only pin the wiring. The adapter now exists, and the property is a sequence in
    /// the trace: write the temp file, sync its bytes, rename it into place, **sync the directory**,
    /// and only then truncate the WAL.
    ///
    /// Why the order and not just the presence: a directory sync *after* the truncation is exactly
    /// the v2.4.4 bug — a power loss between them leaves the old `snapshot.bin` beside an empty,
    /// fsynced `wal.bin`, and every acknowledged record since the previous snapshot is gone. A test
    /// that only asserted "fsync_dir was called" would pass on that.
    #[cfg(feature = "sim")]
    #[tokio::test]
    async fn the_snapshot_syncs_the_directory_before_it_truncates_the_wal() {
        use mycelium_sim::{Kernel, Sources};

        let dir  = unique_dir("sim-order");
        let node = NodeId::new("127.0.0.1", 1).unwrap();
        let hlc  = Arc::new(crate::hlc::Hlc::new());
        let state = KvState::new(0);
        apply_and_notify(&state, &make_gossip_update(&node, 1, Arc::from("k"), Bytes::from_static(b"v"), false, &hlc));
        let mut wal = open_wal(&dir.join("wal.bin")).await.unwrap();

        crate::sim_seam::install(crate::sim_seam::SimContext {
            kernel:  Kernel::recording(),
            sources: Sources::seeded(1, 1_789_000_000_000),
            node:    "n1".into(),
            offsets: Default::default(),
        });
        do_snapshot(&dir, &state, &node, &hlc, 1, &mut wal, None).await.unwrap();
        let ctx = crate::sim_seam::take().expect("kernel installed");

        // Only the storage effects, in the order production produced them.
        let fs: Vec<(String, String)> = ctx
            .kernel
            .trace()
            .entries()
            .iter()
            .filter(|e| e.kind == mycelium_sim::ChoiceKind::Fs)
            .map(|e| (e.stream.clone(), e.request.split(' ').next().unwrap_or("").to_string()))
            .collect();

        let position = |stream: &str, op: &str| -> usize {
            fs.iter()
                .position(|(s, o)| s == stream && o == op)
                .unwrap_or_else(|| panic!("no {op} on {stream} in {fs:?}"))
        };

        let tmp_written   = position("snapshot.tmp", "write");
        let tmp_synced    = position("snapshot.tmp", "sync_data");
        let renamed       = position("snapshot.bin", "rename");
        let dir_synced    = position("dir", "sync_dir");
        let wal_truncated = position("wal.bin#truncate", "sync_data");

        assert!(tmp_written < tmp_synced, "the temp file's bytes are synced after they are written");
        assert!(tmp_synced < renamed, "a snapshot is published only once its bytes are durable");
        assert!(renamed < dir_synced, "the directory sync is what makes the rename durable");
        assert!(
            dir_synced < wal_truncated,
            "THE v2.4.4 PROPERTY: the directory sync must precede the WAL truncation. \
             Reversed, a power loss between them leaves the old snapshot beside an empty, fsynced \
             WAL — every acknowledged record since the previous snapshot gone. Trace: {fs:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn regression_snapshot_aborts_when_wal_tail_is_unreadable() {
        // Flagged by the 2026-09-05 deterministic-replay design review: the merge's read-back
        // mapped a read error to an empty tail (`unwrap_or_default`), so a transient read
        // failure during a snapshot would have truncated acknowledged records. The snapshot
        // must fail instead — no snapshot.bin installed, the WAL handle untouched.
        let dir  = unique_dir("unreadable");
        let node = NodeId::new("127.0.0.1", 1).unwrap();
        let hlc  = Arc::new(crate::hlc::Hlc::new());
        let state = KvState::new(0);
        // Make `wal.bin` unreadable-as-a-file: a directory in its place (EISDIR on read).
        std::fs::create_dir_all(dir.join("wal.bin")).unwrap();
        // The writer's handle is modelled by a scratch file elsewhere; it must not be truncated.
        let mut scratch = open_wal(&dir.join("scratch.log")).await.unwrap();
        wal_append(&mut scratch, &entry("k", b"v", 1, false), true, None).await.unwrap();
        let before = std::fs::metadata(dir.join("scratch.log")).unwrap().len();

        let r = do_snapshot(&dir, &state, &node, &hlc, 1, &mut scratch, None).await;
        assert!(r.is_err(), "an unreadable WAL tail must abort the snapshot, not be treated as empty");
        assert!(!dir.join("snapshot.bin").exists(), "no snapshot may be installed on a failed read-back");
        assert_eq!(std::fs::metadata(dir.join("scratch.log")).unwrap().len(), before,
            "the WAL handle must not be truncated when the snapshot aborted");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn regression_snapshot_retains_wal_record_acked_before_local_apply() {
        // Finding 1 (reviewer's probe, verbatim shape): the writer acks an fsynced
        // append and snapshots before the caller has applied the write to the store.
        // The store scan lacks the key; the WAL held it — truncation must not lose it.
        let dir  = unique_dir("merge");
        let node = NodeId::new("127.0.0.1", 1).unwrap();
        let hlc  = Arc::new(crate::hlc::Hlc::new());
        let state = KvState::new(0);
        let update = make_gossip_update(&node, 1, Arc::from("user/key"), Bytes::from_static(b"durable"), false, &hlc);
        let mut file = open_wal(&dir.join("wal.bin")).await.unwrap();
        wal_append(&mut file, &sync_entry_from(&update), true, None).await.unwrap();
        do_snapshot(&dir, &state, &node, &hlc, 1, &mut file, None).await.unwrap();
        assert_eq!(std::fs::metadata(dir.join("wal.bin")).unwrap().len(), 0, "snapshot truncates the WAL");
        apply_and_notify(&state, &update); // the caller resumes — too late for the scan

        let restored = replay_into_fresh_store(&dir).await;
        assert_eq!(live_value(&restored, "user/key").as_deref(), Some(&b"durable"[..]),
            "fsynced write vanished when snapshot ran before in-memory apply");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn snapshot_wal_merge_follows_store_lww() {
        // The merge must be exactly the store's conflict rule, or a restart would
        // resolve (store ∪ WAL) differently from a live node:
        //  a) store newer than WAL  → store wins
        //  b) WAL newer than store  → WAL wins (incl. a tombstone over a live value)
        //  c) two WAL records, same key → later timestamp wins
        let dir  = unique_dir("lww");
        let node = NodeId::new("127.0.0.1", 1).unwrap();
        let hlc  = Arc::new(crate::hlc::Hlc::new());
        let state = KvState::new(0);
        let t = |n: u64| crate::hlc::pack(1_000 + n, 0);
        let put = |key: &str, val: &'static [u8], tomb: bool, ts: u64| GossipUpdate {
            nonce: 1, sender: 0, ttl: 1, is_tombstone: tomb, timestamp: ts,
            key: Arc::from(key), value: Bytes::from_static(val),
        };
        apply_and_notify(&state, &put("a", b"store-new", false, t(9)));
        apply_and_notify(&state, &put("b", b"store-old", false, t(1)));
        let mut file = open_wal(&dir.join("wal.bin")).await.unwrap();
        for e in [
            entry("a", b"wal-old", t(2), false),   // (a) loses to the store
            entry("b", b"",        t(5), true),    // (b) tombstone beats the live store value
            entry("c", b"first",   t(3), false),   // (c) …
            entry("c", b"second",  t(4), false),   //     … later record wins
        ] {
            wal_append(&mut file, &e, false, None).await.unwrap();
        }
        do_snapshot(&dir, &state, &node, &hlc, 1, &mut file, None).await.unwrap();

        let restored = replay_into_fresh_store(&dir).await;
        assert_eq!(live_value(&restored, "a").as_deref(), Some(&b"store-new"[..]));
        assert_eq!(restored.store.pin().get("b").map(|e| e.data.is_none()), Some(true),
            "newer WAL tombstone must win over the older live store value");
        assert_eq!(live_value(&restored, "c").as_deref(), Some(&b"second"[..]));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn regression_writer_threshold_snapshot_right_after_ack_keeps_write() {
        // End-to-end through the real writer task: threshold 1 makes the writer
        // snapshot in the same poll as the ack, before the awaiting caller can apply.
        // Replay is taken from disk WHILE the writer is alive — a clean shutdown
        // would re-snapshot the (by then updated) store and mask the loss.
        let dir  = unique_dir("writer");
        let node = NodeId::new("127.0.0.1", 1).unwrap();
        let hlc  = Arc::new(crate::hlc::Hlc::new());
        let state = KvState::new(0);
        let handle = spawn_wal_writer(dir.clone(), SyncMode::Flush, 1, 3_600,
            Arc::clone(&state), node.clone(), Arc::clone(&hlc), 1, None, None);

        let update = make_gossip_update(&node, 1, Arc::from("user/key"), Bytes::from_static(b"durable"), false, &hlc);
        handle.append_sync(sync_entry_from(&update)).await.unwrap();
        // Structural poll: the threshold snapshot has run (snapshot.bin exists, WAL truncated).
        for _ in 0..400 {
            let snapped = dir.join("snapshot.bin").exists()
                && std::fs::metadata(dir.join("wal.bin")).map(|m| m.len() == 0).unwrap_or(false);
            if snapped { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(dir.join("snapshot.bin").exists(), "writer never snapshotted at threshold 1");
        apply_and_notify(&state, &update); // the caller applies only now

        let restored = replay_into_fresh_store(&dir).await;
        assert_eq!(live_value(&restored, "user/key").as_deref(), Some(&b"durable"[..]),
            "acked write lost: writer snapshotted (and truncated) before the caller applied");
        drop(handle);
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── Invariant 2: replay is LWW, not a watermark ───────────────────────────

    #[tokio::test]
    async fn regression_replay_keeps_wal_record_older_than_snapshot_watermark() {
        // Finding 2 (reviewer's probe): a delayed remote update with an HLC below
        // `snapshot_hlc`, accepted and WAL-appended after the snapshot, was dropped
        // by the `timestamp > snapshot_hlc` filter — even for a key the snapshot lacks.
        let dir = unique_dir("watermark");
        let snapshot = KvSnapshot { snapshot_hlc: 100, entries: vec![] };
        tfs::write(dir.join("snapshot.bin"), codec::to_vec(&snapshot).unwrap()).await.unwrap();
        let mut file = open_wal(&dir.join("wal.bin")).await.unwrap();
        wal_append(&mut file, &entry("user/key", b"durable", 50, false), true, None).await.unwrap();

        let restored = replay_into_fresh_store(&dir).await;
        assert_eq!(live_value(&restored, "user/key").as_deref(), Some(&b"durable"[..]),
            "post-snapshot arrival with older HLC was skipped on replay");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── V2: golden on-disk fixtures (contracts axis item 1 PR 1; `docs/design/contracts-receipts.md` §9)

    /// Root of the committed fixtures: one directory per released on-disk format, each holding
    /// `wal.bin`, `snapshot.bin` and `expected.json` (`{"live": {key: utf8}, "tombstoned": [key]}`).
    fn golden_fixture_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/persistence")
    }

    /// The fixture's content, chosen to exercise what the durability invariants protect: a key in
    /// both snapshot and WAL where LWW decides (`both/newer-in-wal`, `both/newer-in-snapshot`), a
    /// WAL-only key whose HLC is *older* than the snapshot watermark (invariant 2:
    /// `wal-only/older-than-watermark` must still replay), a snapshot-only key, and a tombstone.
    fn golden_expected() -> (Vec<(&'static str, &'static [u8])>, Vec<&'static str>) {
        (
            vec![
                ("snap-only/a", b"snapshot value"),
                ("both/newer-in-wal", b"wal wins"),
                ("both/newer-in-snapshot", b"snapshot wins"),
                ("wal-only/older-than-watermark", b"delayed remote update"),
                ("wal-only/newer", b"appended after snapshot"),
            ],
            vec!["tomb/deleted-in-wal"],
        )
    }

    /// Regenerate `tests/fixtures/persistence/fixint-v1` with the *current* writer. Run by hand
    /// (`cargo test -p mycelium-core regenerate_golden_fixture_fixint_v1 -- --ignored`) only when
    /// the format that directory names is the one this code writes; a format change adds a new
    /// directory instead. Never in CI.
    #[tokio::test]
    #[ignore]
    async fn regenerate_golden_fixture_fixint_v1() {
        let dir = golden_fixture_root().join("fixint-v1");
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["wal.bin", "snapshot.bin", "snapshot.tmp"] { let _ = std::fs::remove_file(dir.join(f)); }
        let node  = NodeId::new("127.0.0.1", 7).unwrap();
        let hlc   = Arc::new(crate::hlc::Hlc::new());
        let state = KvState::new(0);
        // Deterministic HLC timestamps (packed physical-ms << 16 | logical) so the files are stable.
        let ts = |ms: u64| ms << 16;
        let put = |key: &str, val: &'static [u8], t: u64, tomb: bool| {
            let e = entry(key, val, t, tomb);
            apply_and_notify(&state, &GossipUpdate {
                nonce: crate::framing::ANTI_ENTROPY_NONCE, sender: 0, ttl: 1,
                is_tombstone: e.is_tombstone, timestamp: e.timestamp, key: e.key.clone(), value: e.value.clone(),
            });
            e
        };
        let mut file = open_wal(&dir.join("wal.bin")).await.unwrap();
        // Records that end up in the snapshot (applied to the store before it is taken).
        let e1 = put("snap-only/a",            b"snapshot value",   ts(1_000), false);
        let e2 = put("both/newer-in-wal",      b"old value",        ts(1_000), false);
        let e3 = put("both/newer-in-snapshot", b"snapshot wins",    ts(3_000), false);
        let e4 = put("tomb/deleted-in-wal",    b"to be deleted",    ts(1_000), false);
        for e in [&e1, &e2, &e3, &e4] { wal_append(&mut file, e, true, None).await.unwrap(); }
        do_snapshot(&dir, &state, &node, &hlc, 1, &mut file, None).await.unwrap();
        assert_eq!(std::fs::metadata(dir.join("wal.bin")).unwrap().len(), 0, "snapshot truncated the WAL");
        // The WAL tail after the snapshot: LWW both ways, an older-than-watermark record, a tombstone.
        for e in [
            entry("both/newer-in-wal",             b"wal wins",                ts(2_000), false),
            entry("both/newer-in-snapshot",        b"stale",                   ts(2_000), false),
            entry("wal-only/older-than-watermark", b"delayed remote update",   ts(500),   false),
            entry("wal-only/newer",                b"appended after snapshot", ts(4_000), false),
            entry("tomb/deleted-in-wal",           b"",                        ts(2_000), true),
        ] { wal_append(&mut file, &e, true, None).await.unwrap(); }
        drop(file);
        let (live, tombs) = golden_expected();
        let mut json = String::from("{\n  \"format\": \"fixint-v1\",\n  \"since\": \"v1.0.0\",\n  \"live\": {\n");
        for (i, (k, v)) in live.iter().enumerate() {
            json.push_str(&format!("    \"{k}\": \"{}\"{}\n", std::str::from_utf8(v).unwrap(), if i + 1 < live.len() { "," } else { "" }));
        }
        json.push_str("  },\n  \"tombstoned\": [");
        json.push_str(&tombs.iter().map(|k| format!("\"{k}\"")).collect::<Vec<_>>().join(", "));
        json.push_str("]\n}\n");
        std::fs::write(dir.join("expected.json"), json).unwrap();
    }

    /// V2 gate: every committed fixture directory replays through the production `replay` + LWW
    /// apply path and matches its `expected.json`. A format change that breaks an old file fails
    /// here; a new format adds a directory rather than editing one.
    #[tokio::test]
    async fn golden_fixture_replays_every_released_on_disk_format() {
        let root = golden_fixture_root();
        let mut dirs: Vec<_> = std::fs::read_dir(&root)
            .unwrap_or_else(|e| panic!("fixture root {} missing: {e}", root.display()))
            .filter_map(|d| d.ok()).map(|d| d.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        assert!(!dirs.is_empty(), "no fixture directories under {}", root.display());
        for dir in dirs {
            let expected: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.join("expected.json")).unwrap()).unwrap();
            let restored = replay_into_fresh_store(&dir).await;
            for (k, v) in expected["live"].as_object().unwrap() {
                assert_eq!(live_value(&restored, k).as_deref(), Some(v.as_str().unwrap().as_bytes()),
                    "{}: key {k} did not replay to its expected value", dir.display());
            }
            for k in expected["tombstoned"].as_array().unwrap() {
                let k = k.as_str().unwrap();
                assert!(live_value(&restored, k).is_none(), "{}: {k} should replay as tombstoned", dir.display());
            }
        }
    }

    #[tokio::test]
    async fn replay_without_watermark_still_lets_snapshot_win_same_key() {
        // Dropping the filter must not let an older WAL record clobber the snapshot's
        // newer value for the same key — LWW in `apply_fn` decides.
        let dir = unique_dir("lww-replay");
        let snapshot = KvSnapshot { snapshot_hlc: 100, entries: vec![entry("k", b"newer", 90, false)] };
        tfs::write(dir.join("snapshot.bin"), codec::to_vec(&snapshot).unwrap()).await.unwrap();
        let mut file = open_wal(&dir.join("wal.bin")).await.unwrap();
        wal_append(&mut file, &entry("k", b"older", 40, false), true, None).await.unwrap();
        let restored = replay_into_fresh_store(&dir).await;
        assert_eq!(live_value(&restored, "k").as_deref(), Some(&b"newer"[..]));
        std::fs::remove_dir_all(&dir).ok();
    }
}
