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
                let _ = self.tx.try_send(WalMsg::Append { entry, ack: None, force_sync: false });
                Ok(())
            }
        }
    }

    /// Fire-and-forget for synchronous callers (`set` / `delete`).
    /// Never awaits fsync. Silently drops if the channel is full —
    /// consistent with `GossipAgent::set`'s existing try_send semantics.
    pub fn append_try(&self, entry: SyncEntry) {
        let _ = self.tx.try_send(WalMsg::Append { entry, ack: None, force_sync: false });
    }

    /// Append and await `fdatasync` **regardless of `sync_mode`** — the record is on
    /// stable storage when this returns `Ok`. Used for consensus committed-slot and
    /// lease writes. `Err` if the writer is gone or the append/fsync failed; the
    /// caller must not report durability on `Err`.
    pub async fn append_sync(&self, entry: SyncEntry) -> io::Result<()> {
        self.send_and_await(entry, true).await
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
        match tfs::read(&snapshot_path).await {
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
        match tfs::read(&wal_path).await {
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

    file.write_all(&buf).await?;
    if sync {
        file.sync_data().await?;
    }
    Ok(())
}

// ── Snapshot ─────────────────────────────────────────────────────────────────

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
    let wal_bytes = tfs::read(dir.join("wal.bin")).await.unwrap_or_default();
    if !wal_bytes.is_empty() {
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
    let snap = KvSnapshot { snapshot_hlc, entries };
    let encoded = {
        let buf = codec::to_vec(&snap)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        match cipher {
            Some(c) => c.encrypt(&buf),
            None    => buf,
        }
    };
    tfs::write(&tmp_path, &encoded).await?;
    {
        let f = tfs::File::open(&tmp_path).await?;
        f.sync_data().await?;
    }
    tfs::rename(&tmp_path, &snap_path).await?;

    // 4. Truncate WAL.
    wal_file.seek(std::io::SeekFrom::Start(0)).await?;
    wal_file.set_len(0).await?;
    wal_file.sync_data().await?;

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
