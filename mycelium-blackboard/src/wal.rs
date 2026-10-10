//! Write-ahead log for the board (WS-G / G3 · Phase 2) — durability for the claim discipline.
//!
//! Built against the documented exactly-once-effect contract (`docs/design/exactly-once-effect.md`)
//! and mirroring `mycelium-tuple-space`'s WAL shape: a magic + versioned header (a *newer* format is
//! refused, never silently truncated), length-framed records, torn-tail truncation on open (a
//! corrupt record *with data after it* refuses the open instead), one owner per file
//! (`OwnershipLock`), a writer poisoned by a failed append, a crash-durable compaction (temp file
//! and directory synced), and a compaction epoch. The blackboard's model is simpler than the tuple space's: there are **no
//! stage transitions**, so there is no compound `Complete` record — `Post` / `Claim` / `Ack` /
//! `Release` are each one indivisible record.
//!
//! **Replay liveness.** A fact is live (claimable) iff it was `Post`ed and not `Ack`ed. A
//! claimed-but-unacked fact (the claimer crashed) re-queues to claimable — at-least-once, exactly
//! the tuple space's "taken-but-unacked re-queues as abandoned" rule. `Ack` is the only terminal.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use bytes::Bytes;
use parking_lot::Mutex;

const WAL_MAGIC: &[u8; 6] = b"MBBWAL";
/// v1. The header refuses any version this build does not understand (no silent truncation of a
/// future format); a `PREV_WAL_VERSION` read window opens when v2 ever ships.
const WAL_VERSION: u16 = 1;
const WAL_HEADER_LEN: u64 = 8; // magic(6) + u16 LE version

const REC_POST: u8 = 1;
const REC_CLAIM: u8 = 2;
const REC_ACK: u8 = 3;
const REC_RELEASE: u8 = 4;

fn wal_header() -> [u8; WAL_HEADER_LEN as usize] {
    let mut h = [0u8; WAL_HEADER_LEN as usize];
    h[..6].copy_from_slice(WAL_MAGIC);
    h[6..8].copy_from_slice(&WAL_VERSION.to_le_bytes());
    h
}

/// One WAL record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WalRecord {
    Post { id: u64, attributes: BTreeMap<String, String>, payload: Bytes },
    Claim { id: u64 },
    Ack { id: u64 },
    Release { id: u64 },
}

impl WalRecord {
    pub(crate) fn encode(&self, buf: &mut Vec<u8>) {
        let body_start = buf.len() + 5; // [kind u8][len u32]
        match self {
            WalRecord::Post { id, attributes, payload } => {
                buf.push(REC_POST);
                buf.extend_from_slice(&[0; 4]);
                buf.extend_from_slice(&id.to_le_bytes());
                buf.extend_from_slice(&(attributes.len() as u16).to_le_bytes());
                for (k, v) in attributes {
                    buf.extend_from_slice(&(k.len() as u16).to_le_bytes());
                    buf.extend_from_slice(k.as_bytes());
                    buf.extend_from_slice(&(v.len() as u32).to_le_bytes());
                    buf.extend_from_slice(v.as_bytes());
                }
                buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                buf.extend_from_slice(payload);
            }
            WalRecord::Claim { id } => { buf.push(REC_CLAIM); buf.extend_from_slice(&[0; 4]); buf.extend_from_slice(&id.to_le_bytes()); }
            WalRecord::Ack { id } => { buf.push(REC_ACK); buf.extend_from_slice(&[0; 4]); buf.extend_from_slice(&id.to_le_bytes()); }
            WalRecord::Release { id } => { buf.push(REC_RELEASE); buf.extend_from_slice(&[0; 4]); buf.extend_from_slice(&id.to_le_bytes()); }
        }
        let body_len = (buf.len() - body_start) as u32;
        buf[body_start - 4..body_start].copy_from_slice(&body_len.to_le_bytes());
    }

    /// Decode one record; `None` on a truncated tail.
    fn decode(data: &[u8]) -> Option<(WalRecord, usize)> {
        if data.len() < 5 {
            return None;
        }
        let kind = data[0];
        let body_len = u32::from_le_bytes(data[1..5].try_into().ok()?) as usize;
        let body = data.get(5..5 + body_len)?;
        let rec = match kind {
            REC_POST => {
                let id = u64::from_le_bytes(body.get(..8)?.try_into().ok()?);
                let attr_count = u16::from_le_bytes(body.get(8..10)?.try_into().ok()?) as usize;
                let mut p = 10;
                let mut attributes = BTreeMap::new();
                for _ in 0..attr_count {
                    let kl = u16::from_le_bytes(body.get(p..p + 2)?.try_into().ok()?) as usize;
                    p += 2;
                    let k = std::str::from_utf8(body.get(p..p + kl)?).ok()?.to_string();
                    p += kl;
                    let vl = u32::from_le_bytes(body.get(p..p + 4)?.try_into().ok()?) as usize;
                    p += 4;
                    let v = std::str::from_utf8(body.get(p..p + vl)?).ok()?.to_string();
                    p += vl;
                    attributes.insert(k, v);
                }
                let pl = u32::from_le_bytes(body.get(p..p + 4)?.try_into().ok()?) as usize;
                let payload = body.get(p + 4..p + 4 + pl)?;
                WalRecord::Post { id, attributes, payload: Bytes::copy_from_slice(payload) }
            }
            REC_CLAIM => WalRecord::Claim { id: u64::from_le_bytes(body.get(..8)?.try_into().ok()?) },
            REC_ACK => WalRecord::Ack { id: u64::from_le_bytes(body.get(..8)?.try_into().ok()?) },
            REC_RELEASE => WalRecord::Release { id: u64::from_le_bytes(body.get(..8)?.try_into().ok()?) },
            _ => return None, // unknown kind — treat as corrupt tail
        };
        Some((rec, 5 + body_len))
    }
}

struct WalInner {
    file: File,
    /// `Some(reason)` after a failed append: part of a frame may be on disk, so the file's end is
    /// unknown and nothing may be appended behind it (row C, mirroring the core WAL's `WriterState`).
    /// Every append is refused by name until a compaction rewrites the file from the live state —
    /// or a reopen truncates the torn tail.
    poison: Option<String>,
    /// Consecutive failed repairs, and the `wants_compaction` calls still to decline before the next
    /// attempt — a repair that keeps failing backs off (1, 2, 4 … 64 ticks) rather than rereading
    /// the whole log on every tick.
    repair_failures: u32,
    repair_skip: u32,
    file_len: u64,
    ops_since_sync: u64,
    /// Live records (Post). Terminal records (Ack) bump `acked`; compaction fires past a ratio.
    total: u64,
    acked: u64,
    /// Bumped on every compaction; a replay cursor from a prior epoch must restart.
    epoch: u64,
}

pub(crate) struct WalWriter {
    inner: Mutex<WalInner>,
    path: PathBuf,
    checkpoint_every: u64,
    /// One owner per WAL file: the core's lock on `<wal>.lock`, held for the writer's lifetime.
    _lock: mycelium::OwnershipLock,
    /// Test seam: when set, the next append writes part of its frame and fails — the shape a full
    /// disk or a pulled cable produces, which the production path cannot be made to produce on demand.
    #[cfg(test)]
    pub(crate) fault: std::sync::atomic::AtomicBool,
    /// Test seam: while set, every compaction fails.
    #[cfg(test)]
    pub(crate) compact_fault: std::sync::atomic::AtomicBool,
}

// Test seam: the storage steps compaction takes, in order, on this thread (compaction is synchronous,
// so the test that runs it reads its own trace). Production records nothing.
#[cfg(test)]
thread_local! {
    pub(crate) static FS_TRACE: std::cell::RefCell<Vec<&'static str>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn fs_trace(_op: &'static str) {
    #[cfg(test)]
    FS_TRACE.with(|t| t.borrow_mut().push(_op));
}

/// A live fact recovered from the WAL: `(id, attributes, payload)`.
pub(crate) type LiveFact = (u64, BTreeMap<String, String>, Bytes);

impl WalWriter {
    /// Open (or create) + replay. Returns the writer, the live (claimable) facts in id order, and
    /// the highest id seen (for the `next_id` fence).
    pub(crate) fn open(path: &Path, checkpoint_every: u64) -> io::Result<(Self, Vec<LiveFact>, Option<u64>)> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        // Ownership first, repair second, writes last — the core journal's order. A second board on
        // this path, in this process or another, is refused with `WouldBlock` naming the file.
        let lock = mycelium::OwnershipLock::acquire(path)?;
        let mut file = OpenOptions::new().read(true).create(true).append(true).open(path)?;
        let mut data = Vec::new();
        file.seek(SeekFrom::Start(0))?;
        file.read_to_end(&mut data)?;

        if data.is_empty() {
            file.write_all(&wal_header())?;
            data.extend_from_slice(&wal_header());
        } else if data.len() < WAL_HEADER_LEN as usize || &data[..WAL_MAGIC.len()] != WAL_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is not a mycelium-blackboard WAL (missing MBBWAL magic); refusing to open", path.display()),
            ));
        } else {
            let version = u16::from_le_bytes(data[6..8].try_into().expect("two header bytes"));
            if version != WAL_VERSION {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is WAL format v{version}, but this build supports v{WAL_VERSION}; refusing to open (no silent truncation of newer formats)", path.display()),
                ));
            }
        }

        // Replay. Post adds a fact; Claim/Release are runtime-only (a claimed-unacked fact
        // re-queues as claimable — at-least-once); Ack is the sole terminal.
        let Folded { facts, total, acked, end: offset, high_water } = fold_wal(&data).map_err(|offset| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} holds a corrupt record at byte {offset} with data after it; refusing to open \
                     (the records after it cannot be trusted, and truncating would discard them \
                     silently). The file is untouched: move it aside to start empty, or restore it",
                    path.display()
                ),
            )
        })?;
        if offset < data.len() {
            // Drop the torn tail so the next append lands where every reader will find it, and make
            // the truncation as durable as the file (a crash must not resurrect the torn bytes).
            file.set_len(offset as u64)?;
            file.sync_all()?;
            fsync_parent(path)?;
            tracing::warn!(
                wal = %path.display(),
                dropped_bytes = data.len() - offset,
                "blackboard: truncated a torn final record left by a crash mid-append"
            );
        }
        let max_id = high_water;
        let live: Vec<LiveFact> = facts
            .into_iter()
            .filter(|(_, f)| !f.acked)
            .map(|(id, f)| (id, f.attributes, f.payload))
            .collect();
        let file_len = offset as u64;
        Ok((
            Self {
                inner: Mutex::new(WalInner { file, poison: None, repair_failures: 0, repair_skip: 0, file_len, ops_since_sync: 0, total, acked, epoch: 0 }),
                path: path.to_path_buf(),
                checkpoint_every,
                _lock: lock,
                #[cfg(test)]
                fault: std::sync::atomic::AtomicBool::new(false),
                #[cfg(test)]
                compact_fault: std::sync::atomic::AtomicBool::new(false),
            },
            live,
            max_id,
        ))
    }

    pub(crate) fn append(&self, rec: &WalRecord) -> io::Result<()> {
        let mut g = self.inner.lock();
        let mut buf = Vec::new();
        rec.encode(&mut buf);
        if let Some(reason) = &g.poison {
            return Err(io::Error::other(format!(
                "the blackboard WAL refuses appends after a failed write ({reason}) until a compaction \
                 rewrites it or a reopen truncates the torn tail"
            )));
        }
        #[cfg(test)]
        let written = if self.fault.swap(false, std::sync::atomic::Ordering::SeqCst) {
            let _ = g.file.write_all(&buf[..buf.len().min(5)]);
            Err(io::Error::other("injected write failure after a partial frame"))
        } else {
            g.file.write_all(&buf)
        };
        #[cfg(not(test))]
        let written = g.file.write_all(&buf);
        if let Err(e) = written {
            // Part of the frame may be on disk; nothing may follow it.
            tracing::error!(
                error = %e,
                "blackboard: a WAL append failed; every later append is refused until a compaction \
                 rewrites the log (the next maintenance tick tries one)"
            );
            g.poison = Some(e.to_string());
            return Err(e);
        }
        g.file_len += buf.len() as u64;
        match rec {
            WalRecord::Post { .. } => g.total += 1,
            WalRecord::Ack { .. } => g.acked += 1,
            _ => {}
        }
        g.ops_since_sync += 1;
        if g.ops_since_sync >= self.checkpoint_every {
            g.file.sync_data()?;
            g.ops_since_sync = 0;
        }
        Ok(())
    }

    /// True once acked records dominate — a compaction would reclaim meaningful space.
    pub(crate) fn wants_compaction(&self) -> bool {
        let mut g = self.inner.lock();
        if g.poison.is_some() {
            // A poisoned writer is repaired by the rewrite, so it wants one — unless the last repair
            // failed and the back-off has not run out.
            if g.repair_skip > 0 {
                g.repair_skip -= 1;
                return false;
            }
            return true;
        }
        g.total >= 64 && g.acked * 2 >= g.total
    }

    /// Rewrite the WAL to contain only its live facts — **folded from the log itself**, under the
    /// WAL lock, never from memory — and atomically swap it in. Bumps the epoch.
    ///
    /// Row C: a post appends to the WAL and then applies to memory, so a compaction that snapshot
    /// memory could land between the two and rewrite a log without a post whose append had already
    /// been acknowledged. The log holds every acknowledged record by definition, and no append
    /// interleaves while the lock is held, so the rewrite loses none. A change applied to memory
    /// *before* its append (claim, ack, release, discard) appends to the new file after this
    /// returns. Only the bytes up to `file_len` are folded, so a poisoned writer's torn frame is
    /// left behind.
    pub(crate) fn compact(&self) -> io::Result<()> {
        let mut guard = self.inner.lock();
        let result = self.compact_locked(&mut guard);
        match &result {
            Ok(()) => {
                guard.repair_failures = 0;
                guard.repair_skip = 0;
            }
            Err(e) if guard.poison.is_some() => {
                guard.repair_failures = guard.repair_failures.saturating_add(1);
                guard.repair_skip = 1u32 << guard.repair_failures.min(6);
                tracing::warn!(
                    error = %e,
                    failures = guard.repair_failures,
                    "blackboard: the repairing compaction failed; backing off {} ticks",
                    guard.repair_skip
                );
            }
            Err(_) => {}
        }
        result
    }

    fn compact_locked(&self, g: &mut WalInner) -> io::Result<()> {
        #[cfg(test)]
        if self.compact_fault.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(io::Error::other("injected compaction failure"));
        }
        let mut data = vec![0u8; g.file_len as usize];
        g.file.seek(SeekFrom::Start(0))?;
        g.file.read_exact(&mut data)?;
        let folded = fold_wal(&data).map_err(|offset| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} holds a corrupt record at byte {offset}; compaction refuses to rewrite over it",
                    self.path.display()
                ),
            )
        })?;
        let mut live: Vec<WalRecord> = folded
            .facts
            .into_iter()
            .filter(|(_, f)| !f.acked)
            .map(|(id, f)| WalRecord::Post { id, attributes: f.attributes, payload: f.payload })
            .collect();
        let posts = live.len() as u64;
        // The id high-water mark survives the rewrite (#597 review, finding 7): when the highest id
        // the log ever held was acked away, a lone `Ack` for it is kept, so the next open fences
        // `next_id` past it and a restart never reissues an acknowledged id. A lone `Ack` names no
        // fact, so replay — this build's or an older one's — changes nothing else for it.
        let highest_live = live.iter().map(|r| match r { WalRecord::Post { id, .. } => *id, _ => 0 }).max();
        if let Some(hw) = folded.high_water
            && highest_live.is_none_or(|h| h < hw)
        {
            live.push(WalRecord::Ack { id: hw });
        }
        // Named after the whole WAL file, so two WALs sharing a stem never share a temp file.
        let tmp = {
            let mut name = self.path.as_os_str().to_owned();
            name.push(".compact");
            PathBuf::from(name)
        };
        let mut buf = wal_header().to_vec();
        for rec in &live {
            rec.encode(&mut buf);
        }
        // The core snapshot's install order (`sync_data → rename → fsync_dir`): the temp file's bytes
        // durable before it is published, and the directory synced so the rename itself survives a
        // power loss. `std::fs::write` + `rename` alone could publish an empty or partial log.
        let mut tmp_file = File::create(&tmp)?;
        tmp_file.write_all(&buf)?;
        fs_trace("tmp.write");
        tmp_file.sync_data()?;
        fs_trace("tmp.sync");
        drop(tmp_file);
        std::fs::rename(&tmp, &self.path)?;
        fs_trace("rename");
        fsync_parent(&self.path)?;
        fs_trace("dir.sync");
        let mut file = OpenOptions::new().read(true).append(true).open(&self.path)?;
        file.seek(SeekFrom::End(0))?;
        g.file = file;
        g.file_len = buf.len() as u64;
        g.total = posts;
        g.acked = 0;
        g.ops_since_sync = 0;
        g.epoch += 1;
        // The torn frame went with the old file; appends resume behind the rewritten log.
        if let Some(reason) = g.poison.take() {
            tracing::info!(%reason, "blackboard: a compaction rewrote the WAL after a failed append; appends resume");
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn poisoned(&self) -> bool {
        self.inner.lock().poison.is_some()
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.inner.lock().epoch
    }

    /// Force a durable sync (the periodic checkpoint task).
    pub(crate) fn sync(&self) -> io::Result<()> {
        let mut g = self.inner.lock();
        g.file.sync_data()?;
        g.ops_since_sync = 0;
        Ok(())
    }
}

/// One fact's state as the log records it.
struct FactState {
    attributes: BTreeMap<String, String>,
    payload: Bytes,
    acked: bool,
}

/// A WAL image folded to its facts: what open replays and compaction rewrites.
struct Folded {
    facts: BTreeMap<u64, FactState>,
    total: u64,
    acked: u64,
    /// Where the complete records end — short of the image's end only for a torn final frame.
    end: usize,
    /// The highest fact id any record names, acked or live — what `next_id` is fenced past.
    high_water: Option<u64>,
}

/// Fold a whole WAL image (header included) record by record. `Err(offset)` names a corrupt record
/// with data after it; a torn final frame ends the fold at `end`.
fn fold_wal(data: &[u8]) -> Result<Folded, usize> {
    let mut facts: BTreeMap<u64, FactState> = BTreeMap::new();
    let mut total = 0u64;
    let mut acked = 0u64;
    let mut high_water: Option<u64> = None;
    let mut offset = WAL_HEADER_LEN as usize;
    while offset < data.len() {
        match scan_frame(&data[offset..]) {
            // The image ends inside this frame: a crash mid-append. Nothing in it was acknowledged
            // (a failed append poisons the writer, so nothing lands behind it).
            Frame::Torn => break,
            Frame::Corrupt => return Err(offset),
            Frame::Record(rec, consumed) => {
                offset += consumed;
                let (WalRecord::Post { id, .. } | WalRecord::Claim { id } | WalRecord::Ack { id } | WalRecord::Release { id }) = &rec;
                high_water = high_water.max(Some(*id));
                match rec {
                    WalRecord::Post { id, attributes, payload } => {
                        total += 1;
                        facts.insert(id, FactState { attributes, payload, acked: false });
                    }
                    // Counted only when it terminates a fact the log holds: the lone `Ack` a compaction
                    // keeps for the high-water mark is not a terminal record.
                    WalRecord::Ack { id } => {
                        if let Some(f) = facts.get_mut(&id) {
                            acked += 1;
                            f.acked = true;
                        }
                    }
                    // Claim / Release do not change replay liveness.
                    WalRecord::Claim { .. } | WalRecord::Release { .. } => {}
                }
            }
        }
    }
    Ok(Folded { facts, total, acked, end: offset, high_water })
}

/// What a WAL image holds at a record boundary — the difference between a crash and corruption
/// (row C, after the core's `WalEnd`).
enum Frame {
    /// A whole record, and the bytes it took.
    Record(WalRecord, usize),
    /// The image ends inside this record: a crash mid-append. Everything before it is good.
    Torn,
    /// A record that is all there and does not decode, or bytes that are not a record at all, with
    /// data after them. Nothing after it can be trusted — and nothing after it may be truncated away.
    Corrupt,
}

fn scan_frame(data: &[u8]) -> Frame {
    if let Some((rec, n)) = WalRecord::decode(data) {
        return Frame::Record(rec, n);
    }
    if !(REC_POST..=REC_RELEASE).contains(&data[0]) {
        // Not the start of any record this build writes. Zeros only (preallocation after a crash)
        // read as a torn tail, as in the core; anything else is corruption.
        return if data.iter().any(|b| *b != 0) { Frame::Corrupt } else { Frame::Torn };
    }
    // A valid kind byte and nothing but zeros after it: a partial append the filesystem zero-extended
    // in a crash (#597 review, finding 3) — torn, like the core's zeroed prefix.
    if data.len() < 5 || data[1..].iter().all(|b| *b == 0) {
        return Frame::Torn;
    }
    let body_len = u32::from_le_bytes(data[1..5].try_into().expect("four length bytes")) as usize;
    // A length the image cannot supply is a torn tail; a whole body that does not decode is not.
    // What this cannot see, for want of a checksum: a length prefix corrupted to run past the end of
    // the file reads as torn; and a torn frame whose missing body was zero-filled can *decode* — a
    // phantom record (an `Ack` of id 0, a `Post` with a zeroed payload) replayed as if written. That
    // is why a failed append poisons the writer rather than relying on this scan.
    if body_len.saturating_add(5) > data.len() { Frame::Torn } else { Frame::Corrupt }
}

/// Fsync the directory holding `path`, so a rename or a truncation survives a power loss the way
/// the file's bytes do. Honoured by ext4 / XFS / btrfs; a no-op on non-Unix platforms.
fn fsync_parent(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
        File::open(parent)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trips() {
        let attrs = BTreeMap::from([("k".to_string(), "v".to_string()), ("feeder".to_string(), "4".to_string())]);
        for rec in [
            WalRecord::Post { id: 7, attributes: attrs.clone(), payload: Bytes::from("p") },
            WalRecord::Claim { id: 7 },
            WalRecord::Ack { id: 7 },
            WalRecord::Release { id: 9 },
        ] {
            let mut buf = Vec::new();
            rec.encode(&mut buf);
            let (got, n) = WalRecord::decode(&buf).expect("decodes");
            assert_eq!(got, rec);
            assert_eq!(n, buf.len());
        }
    }

    #[test]
    fn truncated_tail_decodes_to_none() {
        let mut buf = Vec::new();
        WalRecord::Post { id: 1, attributes: BTreeMap::new(), payload: Bytes::from("xyz") }.encode(&mut buf);
        buf.truncate(buf.len() - 2);
        assert!(WalRecord::decode(&buf).is_none());
    }
}
