//! The in-memory board store (WS-G / G3 · Phase 1) — the pure claim-by-predicate primitive.
//!
//! `BoardStore` is the content-plane analogue of `mycelium-tuple-space`'s `TupleStore`, but where
//! the tuple space routes by lane *position*, the board routes by *content*: a [`Predicate`] over
//! fact attributes. It embodies the **exactly-once-effect discipline** (single-owner claim,
//! idempotent terminal ack, bounded in-flight with crash-requeue) documented in
//! `docs/design/exactly-once-effect.md` — this crate is that contract's *second* real user.
//!
//! Unlike the tuple space's blocking `take`, `claim` is **non-blocking**: a competitive claim either
//! wins a matching fact now or returns `None` (the loser's empty claim). There are no parked
//! waiters — readiness is expressed by re-claiming, not by blocking.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use parking_lot::Mutex;

use crate::wal::{Replayed, Tombstones, WalRecord, WalWriter};

#[cfg(test)]
thread_local! {
    static COMPACT_BETWEEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
use crate::{BlackboardError, Fact, Predicate};

/// The last base [`fresh_id_base`] handed out in this process, so two stores created in the same
/// millisecond still get disjoint bases.
static LAST_FRESH_BASE: AtomicU64 = AtomicU64::new(0);

/// The first id of an id space with **no history** — a transient board, or a WAL that names no id
/// (ER3, R4-01). A fact's id is its identity everywhere: a mirror's `admit`, the gateway's JSON `id`, and any
/// worker that keys an effect on it. A space that started
/// at 0 reissued its previous life's ids after a restart, and a destination answered `Replayed` for
/// new work. The base is the wall clock's milliseconds shifted left by 10 (read through
/// `mycelium::sim_seam::wall_now_ms`), strictly increasing in-process by at least 1 024 per store.
///
/// **The assumption:** two fresh starts of one namespace whose clocks read within *N*/1024 ms of each
/// other (*N* the ids the earlier one issued) can overlap — a clock stepped back, or a device with no
/// real-time clock booting at a repeatable time. In one process the bump is 1 024 per store, not per
/// id, and *N* stores created in one process push the last base about *N* ms ahead of the clock. A
/// WAL with history never calls this: it fences at `max_id + 1`. Ids stay below 2^53 until about the
/// year 2248, so they are exact as JSON numbers.
///
/// **Keep in sync with `fresh_id_base` in `mycelium-tuple-space/src/store.rs`** — the same rule, copied, each
/// crate with its own static (the two id spaces are independent). It is not moved into `mycelium`
/// because the companions reach only its public API, and this is not part of it.
pub(crate) fn fresh_id_base() -> u64 {
    fresh_id_base_after(&LAST_FRESH_BASE)
}

/// [`fresh_id_base`] against a given process-wide mark: the clock read and the in-process bump. A
/// test passes a mark of its own, at 0, to model a new process without touching the shared static
/// other tests in the binary use.
fn fresh_id_base_after(last: &AtomicU64) -> u64 {
    let now = mycelium::sim_seam::wall_now_ms() << 10;
    let mut prev = last.load(Ordering::Relaxed);
    loop {
        let next = now.max(prev + (1 << 10));
        match last.compare_exchange(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(p) => prev = p,
        }
    }
}

/// One id's contribution to the live-set digest (ER6 PR B): the splitmix64 finaliser, so the xor of
/// a set's mixes changes when any one id does.
pub(crate) fn digest_mix(id: u64) -> u64 {
    let mut z = id.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// An in-flight (claimed-but-not-terminal) fact and when it was claimed (for the deadline sweep).
struct Inflight {
    fact: Fact,
    claimed_at: Instant,
}

/// All mutable state under one lock — eliminates any TOCTOU between the predicate scan and the
/// claim, which is what makes a claim atomically single-owner.
struct BoardInner {
    /// Claimable facts, id-ordered so `claim` resolves ties to the oldest matching fact (FIFO-fair
    /// across the content predicate).
    available: BTreeMap<u64, Fact>,
    /// Claimed facts awaiting `ack` (terminal) or `release` / deadline-requeue (back to available).
    inflight: HashMap<u64, Inflight>,
    /// The acked ids this board remembers (ER6, R11-01) — recorded by `ack` and by `discard`, rebuilt
    /// from the WAL at open, sent to a mirror with the snapshot, and refused by `post_with_id`. The
    /// one acked-id memory a board has; the mirror's own index went with it.
    tombstones: Tombstones,
}

/// What [`BoardStore::apply_with_id`] did with a replicated fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PostOutcome {
    /// Applied: the fact is now live here.
    Applied,
    /// Already held (or acked while it was being applied): nothing changed.
    Held,
    /// Refused: the id is one this board remembers acked.
    Refused,
}

/// What a mirror needs from a primary's snapshot, read under one lock (ER6): the live facts, the
/// id the primary issues next, and the acked ids it remembers as sorted runs.
pub(crate) struct MirrorSnapshot {
    pub(crate) facts: Vec<Fact>,
    pub(crate) next_id: u64,
    pub(crate) acked: Vec<(u64, u32)>,
}

/// Live depth snapshot for one board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardDepth {
    /// Facts currently claimable.
    pub available: u64,
    /// Facts currently claimed and awaiting a terminal ack.
    pub inflight: u64,
}

/// Cumulative counters for one board (the `sys/bb/{node}/{board}/…` posture).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardStats {
    pub posted: u64,
    pub claimed: u64,
    pub acked: u64,
    pub released: u64,
    pub requeued: u64,
    /// `post`s refused at admission because the pool stood at the high watermark (item 4 PR 5;
    /// v2.8.0). Reported beside the others: a rejection is a visible outcome, not a silence.
    pub rejected: u64,
}

/// The pure, in-memory board: typed facts with non-destructive [`read`](Self::read) and competitive
/// destructive [`claim`](Self::claim). Cheap to construct and exercise without a cluster.
pub struct BoardStore {
    inner: Mutex<BoardInner>,
    next_id: AtomicU64,
    posted: AtomicU64,
    rejected: AtomicU64,
    /// The admission bound on `available`; `None` = unbounded.
    high_watermark: Option<u64>,
    claimed: AtomicU64,
    acked: AtomicU64,
    released: AtomicU64,
    requeued: AtomicU64,
    /// Replicated `post_with_id`s refused because the id is a remembered acked id (ER6) — a
    /// reused id from another primary, or a stale re-delivery. Detection: never a re-sync.
    refused_acked: AtomicU64,
    /// `Some` for a persistent (WAL-backed) board; `None` for a transient one.
    wal: Option<WalWriter>,
}

impl Default for BoardStore {
    fn default() -> Self {
        Self::transient()
    }
}

impl BoardStore {
    /// A transient (in-memory, no WAL) board. Cheap; for tests and non-durable boards.
    pub fn transient() -> Self {
        Self::with_wal(None)
    }

    /// A WAL-backed board: replays `path` (creating it if absent), recovering all live (claimable)
    /// facts and fencing `next_id`. `checkpoint_every` appends between `fdatasync`s.
    pub fn persistent(path: impl AsRef<Path>, checkpoint_every: u64) -> Result<Self, BlackboardError> {
        let (wal, Replayed { live, max_id, tombstones }) = WalWriter::open(path.as_ref(), checkpoint_every)?;
        let store = Self::with_wal(Some(wal));
        // A WAL that names ids fences at its highest + 1; one that names none keeps the fresh base
        // `with_wal` gave it (a deleted WAL must not recycle ids, ER3).
        if let Some(m) = max_id {
            store.next_id.store(m + 1, Ordering::Relaxed);
        }
        {
            let mut g = store.inner.lock();
            g.tombstones = tombstones;
            for (id, attributes, payload) in live {
                g.available.insert(id, Fact { id, attributes, payload });
                store.posted.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(store)
    }

    /// Takes a fresh id base for every board, so a persistent open over a WAL with history takes one
    /// and discards it (`persistent` overwrites `next_id` with the fence). Harmless — the only cost is
    /// the in-process mark moving on by 1 024 — and left so: one constructor, one place the base is set.
    fn with_wal(wal: Option<WalWriter>) -> Self {
        Self {
            inner: Mutex::new(BoardInner { available: BTreeMap::new(), inflight: HashMap::new(), tombstones: Tombstones::default() }),
            // No id history yet: start past every id an earlier life could have issued (ER3).
            next_id: AtomicU64::new(fresh_id_base()),
            posted: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            high_watermark: None,
            claimed: AtomicU64::new(0),
            acked: AtomicU64::new(0),
            released: AtomicU64::new(0),
            requeued: AtomicU64::new(0),
            refused_acked: AtomicU64::new(0),
            wal,
        }
    }

    /// Post a fact (Linda `out`). **Non-destructive** — it joins the claimable pool and, in the
    /// agent-backed board (later phases), gossips to every reader. Returns the fact id.
    /// Bound admission: a `post` that finds `available` at or past `high_watermark` is refused and
    /// counted. `None` leaves the pool unbounded. Replication (`post_with_id`) and WAL replay are
    /// not admission and never refuse.
    pub fn with_high_watermark(mut self, high_watermark: Option<u64>) -> Self {
        self.high_watermark = high_watermark;
        self
    }

    pub fn post(&self, attributes: BTreeMap<String, String>, payload: Bytes) -> Result<u64, BlackboardError> {
        if let Some(high_watermark) = self.high_watermark {
            // One short lock for the count, released before the WAL append (row 27 stays a leaf).
            // The check and the insert are not one step: a self-imposed bound, not a hard one.
            let available = self.inner.lock().available.len() as u64;
            if available >= high_watermark {
                self.rejected.fetch_add(1, Ordering::Relaxed);
                return Err(BlackboardError::Backpressure { available, high_watermark });
            }
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Some(wal) = &self.wal {
            wal.append(&WalRecord::Post { id, attributes: attributes.clone(), payload: payload.clone() })?;
        }
        #[cfg(test)]
        self.between_append_and_apply();
        self.inner.lock().available.insert(id, Fact { id, attributes, payload });
        self.posted.fetch_add(1, Ordering::Relaxed);
        Ok(id)
    }

    /// Test seam: the window between a write's two steps — its WAL append and its in-memory apply,
    /// in whichever order the write takes them. A test arms `COMPACT_BETWEEN` to run a compaction
    /// exactly there, on this thread.
    #[cfg(test)]
    fn between_append_and_apply(&self) {
        if COMPACT_BETWEEN.with(|c| c.replace(false)) {
            self.compact().expect("the interleaved compaction");
        }
    }

    /// Apply a fact under its ORIGINAL id (replication), fencing `next_id` past it. With
    /// [`fence_past`](Self::fence_past) this keeps the rule that **an issued id is never reissued,
    /// live or acked** (ER6, R11-04).
    ///
    /// An id this board holds already is a no-op (a re-delivery, or a snapshot re-posting a fact the
    /// WAL restored). An id it remembers acked is **refused**: nothing is applied or logged, it is
    /// counted in [`refused_acked`](Self::refused_acked), and `Ok` is returned — a mirror must not
    /// bring back a completed fact (ER6). An ack that lands between the check and the insert wins:
    /// the fact is not inserted and an `Ack` follows its `Post` in the log.
    pub fn post_with_id(&self, id: u64, attributes: BTreeMap<String, String>, payload: Bytes) -> Result<(), BlackboardError> {
        self.apply_with_id(id, attributes, payload).map(|_| ())
    }

    /// [`post_with_id`](Self::post_with_id), saying what it did — the refusal decided under the
    /// same lock that checks it, so a caller never counts an id refused that was applied (review
    /// round 2: a separate `remembers_acked` read could race an eviction from the memory).
    pub(crate) fn apply_with_id(&self, id: u64, attributes: BTreeMap<String, String>, payload: Bytes) -> Result<PostOutcome, BlackboardError> {
        self.next_id.fetch_max(id.saturating_add(1), Ordering::Relaxed);
        {
            let g = self.inner.lock();
            if g.tombstones.contains(id) {
                drop(g);
                self.refused_acked.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(id, "blackboard: refused a replicated fact under an id this board remembers acked");
                return Ok(PostOutcome::Refused);
            }
            if g.available.contains_key(&id) || g.inflight.contains_key(&id) {
                return Ok(PostOutcome::Held);
            }
        }
        if let Some(wal) = &self.wal {
            wal.append(&WalRecord::Post { id, attributes: attributes.clone(), payload: payload.clone() })?;
        }
        let acked_meanwhile = {
            let mut g = self.inner.lock();
            let acked = g.tombstones.contains(id);
            if !acked {
                g.available.insert(id, Fact { id, attributes, payload });
            }
            acked
        };
        if acked_meanwhile {
            if let Some(wal) = &self.wal {
                // Refused, the log replays the fact live — at-least-once, never a loss.
                wal.append(&WalRecord::Ack { id })?;
            }
            return Ok(PostOutcome::Held);
        }
        self.posted.fetch_add(1, Ordering::Relaxed);
        Ok(PostOutcome::Applied)
    }

    /// Fence `next_id` at `n` or above (ER6, R11-04): a mirror told the primary's next id never
    /// issues one the primary already did, though the snapshot named only live ids. When the fence
    /// moves on a WAL-backed board it is persisted as a lone `Claim { n - 1 }` — set, then logged — a
    /// record every build folds into the high-water mark and that never changes a fact's liveness
    /// (compaction keeps it a `Claim`, never a tombstone). Returns whether the fence moved; an `n` at
    /// or below the current next id changes nothing and logs nothing.
    pub fn fence_past(&self, n: u64) -> Result<bool, BlackboardError> {
        let prev = self.next_id.fetch_max(n, Ordering::Relaxed);
        if prev >= n {
            return Ok(false);
        }
        if let Some(wal) = &self.wal {
            wal.append(&WalRecord::Claim { id: n - 1 })?;
        }
        Ok(true)
    }

    /// The id this board issues next (ER6 PR B: the fence a digest answer carries).
    pub(crate) fn mirror_snapshot_next_id(&self) -> u64 {
        self.next_id.load(Ordering::Relaxed)
    }

    /// Whether this board remembers `id` as acked (ER6).
    pub fn remembers_acked(&self, id: u64) -> bool {
        self.inner.lock().tombstones.contains(id)
    }

    /// How many acked ids this board remembers (ER6; bounded at 100 000, oldest forgotten first).
    pub fn acked_memory_len(&self) -> usize {
        self.inner.lock().tombstones.len()
    }

    /// Replicated posts refused because their id is a remembered acked id (ER6) — counted, never
    /// a re-sync: no snapshot can cure a reused id.
    pub fn refused_acked(&self) -> u64 {
        self.refused_acked.load(Ordering::Relaxed)
    }

    /// **Non-destructive read** (Linda `rd`): every currently-claimable fact matching `predicate`.
    /// Concurrent and shared — many readers see the same fact. A claimed (in-flight) fact is *not*
    /// returned: it is being consumed by its owner, and reappears only if released or re-queued.
    pub fn read(&self, predicate: &Predicate) -> Vec<Fact> {
        self.inner
            .lock()
            .available
            .values()
            .filter(|f| predicate.matches(&f.attributes))
            .cloned()
            .collect()
    }

    /// **Competitive destructive claim** (Linda `in`): atomically move the oldest claimable fact
    /// matching `predicate` into in-flight and return it; `None` if none match. The whole operation
    /// holds one lock, so two racing claims can never both win the same fact — exactly one gets it,
    /// the other sees it already gone. The returned [`Fact::id`] is the claim handle for
    /// [`ack`](Self::ack) / [`release`](Self::release).
    pub fn claim(&self, predicate: &Predicate) -> Result<Option<Fact>, BlackboardError> {
        let mut g = self.inner.lock();
        let Some(id) = g
            .available
            .iter()
            .find(|(_, f)| predicate.matches(&f.attributes))
            .map(|(id, _)| *id)
        else {
            return Ok(None);
        };
        let fact = g.available.remove(&id).expect("just found");
        g.inflight.insert(id, Inflight { fact: fact.clone(), claimed_at: Instant::now() });
        drop(g); // release before WAL I/O
        if let Some(wal) = &self.wal {
            wal.append(&WalRecord::Claim { id })?;
        }
        self.claimed.fetch_add(1, Ordering::Relaxed);
        Ok(Some(fact))
    }

    /// **Terminal ack**: the claimed fact was consumed — it is gone for good. The exactly-once dedup
    /// point: a duplicate ack is a `NotFound`, never a second effect.
    pub fn ack(&self, id: u64) -> Result<(), BlackboardError> {
        {
            let mut g = self.inner.lock();
            if g.inflight.remove(&id).is_none() {
                return Err(BlackboardError::NotFound);
            }
            // Remembered under the lock the claim state is under (ER6): a mirror's snapshot told
            // this id cannot also be told it is live.
            g.tombstones.insert(id);
        }
        #[cfg(test)]
        self.between_append_and_apply();
        if let Some(wal) = &self.wal {
            wal.append(&WalRecord::Ack { id })?;
        }
        self.acked.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// **Release**: abandon a claim — return the fact to the claimable pool (the loser/abort path,
    /// e.g. an executor that decided not to act). Re-readable and re-claimable afterwards.
    pub fn release(&self, id: u64) -> Result<(), BlackboardError> {
        {
            let mut g = self.inner.lock();
            match g.inflight.remove(&id) {
                Some(Inflight { fact, .. }) => { g.available.insert(id, fact); }
                None => return Err(BlackboardError::NotFound),
            }
        }
        if let Some(wal) = &self.wal {
            wal.append(&WalRecord::Release { id })?;
        }
        self.released.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// True once the WAL has accumulated enough acked records to be worth compacting.
    pub fn wants_compaction(&self) -> bool {
        self.wal.as_ref().is_some_and(WalWriter::wants_compaction)
    }

    /// Rewrite the WAL to hold only live (claimable + in-flight) facts, folded from the log itself
    /// under the WAL lock alone (row C: a snapshot of memory could miss a post appended but not yet
    /// applied). Both replay as claimable, so each is written as a `Post` record. No-op on a
    /// transient board.
    pub fn compact(&self) -> Result<(), BlackboardError> {
        let Some(wal) = &self.wal else { return Ok(()) };
        wal.compact()?;
        Ok(())
    }

    /// One maintenance pass: the periodic sync, then a compaction when one is wanted. Each failure
    /// is the next tick's to retry; neither stops the other.
    pub(crate) fn maintenance_tick(&self) {
        let _ = self.sync();
        if self.wants_compaction() {
            let _ = self.compact();
        }
    }

    /// Test seams over the WAL.
    #[cfg(test)]
    pub(crate) fn inject_append_fault(&self) {
        self.wal.as_ref().unwrap().fault.store(true, Ordering::SeqCst);
    }
    #[cfg(test)]
    pub(crate) fn set_compact_fault(&self, on: bool) {
        self.wal.as_ref().unwrap().compact_fault.store(on, Ordering::SeqCst);
    }
    /// True while the WAL refuses appends (poisoned by a failed write). `false` when transient.
    pub(crate) fn wal_refusing(&self) -> bool {
        self.wal.as_ref().is_some_and(WalWriter::poisoned)
    }
    #[cfg(test)]
    pub(crate) fn wal_poisoned(&self) -> bool {
        self.wal_refusing()
    }
    #[cfg(test)]
    pub(crate) fn set_post_rename_fault(&self, on: bool) {
        self.wal.as_ref().unwrap().post_rename_fault.store(on, Ordering::SeqCst);
    }

    /// The WAL compaction epoch (0 for a transient board) — for tests / future replay cursors.
    pub fn wal_epoch(&self) -> u64 {
        self.wal.as_ref().map_or(0, WalWriter::epoch)
    }

    /// Force a durable WAL sync (the periodic checkpoint task; no-op when transient).
    pub fn sync(&self) -> Result<(), BlackboardError> {
        if let Some(wal) = &self.wal {
            wal.sync()?;
        }
        Ok(())
    }

    /// The mirror's sync source (ER6): every live fact, the next id, and the remembered acked ids,
    /// under one lock — `next_id` read after the facts, so it is past every id they name.
    pub(crate) fn mirror_snapshot(&self) -> MirrorSnapshot {
        let g = self.inner.lock();
        let facts = g.available.values().cloned().chain(g.inflight.values().map(|i| i.fact.clone())).collect();
        let acked = g.tombstones.ranges();
        drop(g);
        MirrorSnapshot { facts, next_id: self.next_id.load(Ordering::Relaxed), acked }
    }

    /// The live-set digest (ER6 PR B): how many live facts, and the xor of each id's
    /// [`digest_mix`], skipping `exclude` (a mirror's unvouched facts).
    pub(crate) fn live_digest(&self, exclude: &std::collections::HashSet<u64>) -> (u64, u64) {
        let g = self.inner.lock();
        let mut count = 0u64;
        let mut mix = 0u64;
        for id in g.available.keys().chain(g.inflight.keys()) {
            if !exclude.contains(id) {
                count += 1;
                mix ^= digest_mix(*id);
            }
        }
        (count, mix)
    }

    /// Every live fact id (claimable + in-flight).
    pub(crate) fn live_ids(&self) -> Vec<u64> {
        let g = self.inner.lock();
        g.available.keys().chain(g.inflight.keys()).copied().collect()
    }

    /// Snapshot of every live fact (claimable + in-flight).
    pub fn snapshot_live(&self) -> Vec<Fact> {
        let g = self.inner.lock();
        g.available
            .values()
            .cloned()
            .chain(g.inflight.values().map(|i| i.fact.clone()))
            .collect()
    }

    /// **Mirror-side terminal**: drop fact `id` (consumed on the primary) from this mirror,
    /// WAL-ing the `Ack` so the mirror's own replay agrees. Returns whether it was present.
    ///
    /// **Since ER6 the id is remembered as acked whether or not the fact was present**, so a later
    /// `post_with_id` of it is refused (an `Ack` that overtook its `Post`, a snapshot's tombstone for
    /// a fact this mirror never held). The `Ack` is logged when the fact was present or the id is new
    /// to the memory — a re-sync re-applying known tombstones writes nothing; `stats().acked` counts
    /// only a fact removed.
    pub fn discard(&self, id: u64) -> Result<bool, BlackboardError> {
        let (removed, fresh) = {
            let mut g = self.inner.lock();
            let removed = g.available.remove(&id).is_some() || g.inflight.remove(&id).is_some();
            (removed, g.tombstones.insert(id))
        };
        if (removed || fresh)
            && let Some(wal) = &self.wal
        {
            wal.append(&WalRecord::Ack { id })?;
        }
        if removed {
            self.acked.fetch_add(1, Ordering::Relaxed);
        }
        Ok(removed)
    }

    /// **Crash-requeue** (at-least-once): in-flight claims older than `timeout` return to the
    /// claimable pool, so a claimer that dropped mid-work does not strand the fact. Returns the
    /// re-queued ids. (Phase 2 drives this on a cadence with the WAL; the mechanism lives here.)
    pub fn requeue_expired(&self, timeout: Duration) -> Vec<u64> {
        let mut g = self.inner.lock();
        let expired: Vec<u64> = g
            .inflight
            .iter()
            .filter(|(_, i)| i.claimed_at.elapsed() >= timeout)
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            if let Some(Inflight { fact, .. }) = g.inflight.remove(id) {
                g.available.insert(*id, fact);
            }
        }
        if !expired.is_empty() {
            self.requeued.fetch_add(expired.len() as u64, Ordering::Relaxed);
        }
        expired
    }

    /// Live depth (claimable + in-flight).
    pub fn depth(&self) -> BoardDepth {
        let g = self.inner.lock();
        BoardDepth {
            available: g.available.len() as u64,
            inflight: g.inflight.len() as u64,
        }
    }

    /// Cumulative counters.
    pub fn stats(&self) -> BoardStats {
        BoardStats {
            posted: self.posted.load(Ordering::Relaxed),
            claimed: self.claimed.load(Ordering::Relaxed),
            acked: self.acked.load(Ordering::Relaxed),
            released: self.released.load(Ordering::Relaxed),
            requeued: self.requeued.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn surplus(feeder: &str, kwh: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("kind".to_string(), "surplus".to_string()),
            ("feeder".to_string(), feeder.to_string()),
            ("kwh".to_string(), kwh.to_string()),
        ])
    }

    #[test]
    fn predicate_equality_and_presence() {
        let f = surplus("4", "3.2");
        assert!(Predicate::new().eq("kind", "surplus").matches(&f));
        assert!(Predicate::new().eq("feeder", "4").present("kwh").matches(&f));
        assert!(!Predicate::new().eq("feeder", "9").matches(&f), "wrong value");
        assert!(!Predicate::new().present("price").matches(&f), "absent attr");
        assert!(Predicate::new().matches(&f), "empty predicate matches all");
    }

    /// Item 4 PR 5: a bounded board refuses a `post` at the watermark, counts it beside the other
    /// counters, and admits again once a claim makes room; replication (`post_with_id`) is not
    /// admission and never refuses; an unbounded board (the default) never refuses.
    #[test]
    fn post_is_refused_and_counted_at_the_high_watermark_but_replication_never_is() {
        let store = BoardStore::transient().with_high_watermark(Some(2));
        store.post(surplus("1", "1.0"), Bytes::new()).unwrap();
        store.post(surplus("2", "1.0"), Bytes::new()).unwrap();
        match store.post(surplus("3", "1.0"), Bytes::new()) {
            Err(BlackboardError::Backpressure { available, high_watermark }) => assert_eq!((available, high_watermark), (2, 2)),
            other => panic!("the third post must be refused at the watermark, got {other:?}"),
        }
        assert_eq!((store.stats().posted, store.stats().rejected), (2, 1), "reported beside the others");
        // Replication is not admission: it lands past the watermark and is never refused.
        store.post_with_id(900, surplus("9", "1.0"), Bytes::new()).unwrap();
        assert_eq!((store.depth().available, store.stats().rejected), (3, 1));
        // Claims make room; the next post is admitted; the count of refusals stays.
        let _ = claim_one(&store, &Predicate::new().eq("feeder", "1"));
        let _ = claim_one(&store, &Predicate::new().eq("feeder", "2"));
        store.post(surplus("4", "1.0"), Bytes::new()).expect("room again");
        assert_eq!((store.stats().posted, store.stats().rejected), (4, 1));

        let unbounded = BoardStore::transient();
        for i in 0..50 {
            unbounded.post(surplus(&i.to_string(), "1.0"), Bytes::new()).expect("unbounded never refuses");
        }
        assert_eq!(unbounded.stats().rejected, 0);
    }

    fn claim_one(store: &BoardStore, pred: &Predicate) -> Fact {
        store.claim(pred).unwrap().expect("a matching fact")
    }

    fn temp_wal(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("mbb-wal-{}-{}.log", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        p
    }

    // ── G-G3.1: competitive exactly-once claim ───────────────────────────────

    #[test]
    fn read_is_non_destructive_claim_is_destructive() {
        let store = BoardStore::transient();
        store.post(surplus("4", "3.2"), Bytes::from("payload")).unwrap();
        let pred = Predicate::new().eq("kind", "surplus");

        // Many non-destructive reads all see the fact.
        assert_eq!(store.read(&pred).len(), 1);
        assert_eq!(store.read(&pred).len(), 1, "read does not consume");
        assert_eq!(store.depth().available, 1);

        // A claim removes it from the claimable pool (and from read).
        let claimed = claim_one(&store, &pred);
        assert_eq!(claimed.payload.as_ref(), b"payload");
        assert_eq!(store.read(&pred).len(), 0, "claimed fact is no longer readable");
        assert_eq!(store.depth(), BoardDepth { available: 0, inflight: 1 });
    }

    #[test]
    fn two_claims_over_one_finite_fact_exactly_one_wins() {
        let store = BoardStore::transient();
        store.post(surplus("4", "3.2"), Bytes::from("the surplus")).unwrap();
        let pred = Predicate::new().eq("kind", "surplus");

        // Two executors race for the single finite fact (sequential — the lock serialises them).
        let a = store.claim(&pred).unwrap();
        let b = store.claim(&pred).unwrap();
        assert!(a.is_some() ^ b.is_some(), "exactly one claim wins; the loser gets None");
        assert!(b.is_none(), "the second claimer sees the fact already gone");
        assert_eq!(store.stats().claimed, 1);
    }

    #[test]
    fn concurrent_claimers_never_double_claim() {
        // The same property under real contention: N threads claim against one fact.
        let store = Arc::new(BoardStore::transient());
        store.post(surplus("4", "3.2"), Bytes::from("x")).unwrap();
        let pred = Predicate::new().eq("kind", "surplus");

        let wins = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut handles = Vec::new();
        for _ in 0..16 {
            let (s, p, w) = (Arc::clone(&store), pred.clone(), Arc::clone(&wins));
            handles.push(std::thread::spawn(move || {
                if s.claim(&p).unwrap().is_some() {
                    w.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(wins.load(Ordering::Relaxed), 1, "exactly one of 16 racers claims the single fact");
        assert_eq!(store.depth().inflight, 1);
    }

    #[test]
    fn release_returns_fact_to_claimable() {
        let store = BoardStore::transient();
        let id = store.post(surplus("4", "3.2"), Bytes::from("x")).unwrap();
        let pred = Predicate::new().eq("kind", "surplus");

        let claimed = claim_one(&store, &pred);
        assert_eq!(claimed.id, id);
        assert_eq!(store.read(&pred).len(), 0);

        store.release(claimed.id).unwrap();
        assert_eq!(store.read(&pred).len(), 1, "released fact is claimable again");
        assert!(store.claim(&pred).unwrap().is_some(), "and re-claimable");
        assert_eq!(store.stats().released, 1);
    }

    #[test]
    fn ack_is_terminal_and_idempotent() {
        let store = BoardStore::transient();
        store.post(surplus("4", "3.2"), Bytes::from("x")).unwrap();
        let pred = Predicate::new().eq("kind", "surplus");

        let claimed = claim_one(&store, &pred);
        store.ack(claimed.id).unwrap();
        assert_eq!(store.depth(), BoardDepth { available: 0, inflight: 0 }, "acked fact is gone");
        // Duplicate ack is a no-op error, never a second effect (the dedup point).
        assert!(matches!(store.ack(claimed.id), Err(BlackboardError::NotFound)));
        assert_eq!(store.read(&pred).len(), 0, "acked fact does not return");
    }

    #[test]
    fn requeue_expired_returns_inflight_claims() {
        let store = BoardStore::transient();
        store.post(surplus("4", "3.2"), Bytes::from("x")).unwrap();
        let pred = Predicate::new().eq("kind", "surplus");

        let claimed = claim_one(&store, &pred);
        // Nothing expired yet at a long timeout.
        assert!(store.requeue_expired(Duration::from_secs(3600)).is_empty());
        // A zero timeout treats the live claim as expired → re-queued.
        let requeued = store.requeue_expired(Duration::from_millis(0));
        assert_eq!(requeued, vec![claimed.id]);
        assert_eq!(store.read(&pred).len(), 1, "the abandoned claim is claimable again");
        assert_eq!(store.stats().requeued, 1);
    }

    #[test]
    fn claim_resolves_to_oldest_matching_fact() {
        let store = BoardStore::transient();
        let first = store.post(surplus("4", "1.0"), Bytes::from("a")).unwrap();
        store.post(surplus("4", "2.0"), Bytes::from("b")).unwrap();
        // Of two matching facts, the oldest (lowest id) is claimed first (FIFO-fair).
        let got = claim_one(&store, &Predicate::new().eq("feeder", "4"));
        assert_eq!(got.id, first);
    }

    // ── G-G3.2: WAL durability (Phase 2) ─────────────────────────────────────

    #[test]
    fn posted_facts_survive_wal_replay() {
        let path = temp_wal("post-replay");
        let pred = Predicate::new().eq("kind", "surplus");
        let first;
        {
            let store = BoardStore::persistent(&path, 1).unwrap();
            // Relative to the first id: a fresh WAL's id space does not start at 0 (ER3).
            first = store.post(surplus("4", "1.0"), Bytes::from("a")).unwrap();
            store.post(surplus("4", "2.0"), Bytes::from("b")).unwrap();
        }
        // Reopen — both facts replay as claimable, ids fenced.
        let store = BoardStore::persistent(&path, 1).unwrap();
        assert_eq!(store.read(&pred).len(), 2);
        let fresh = store.post(surplus("4", "3.0"), Bytes::from("c")).unwrap();
        assert!(fresh >= first + 2, "next_id fenced past replayed ids");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn claimed_but_unacked_fact_requeues_on_replay() {
        // The worked-example "winner drops mid-charge" path: a claim with no ack is at-least-once —
        // on restart the fact returns to claimable so another executor finishes the work.
        let path = temp_wal("claim-crash");
        let pred = Predicate::new().eq("kind", "surplus");
        {
            let store = BoardStore::persistent(&path, 1).unwrap();
            store.post(surplus("4", "3.2"), Bytes::from("finite surplus")).unwrap();
            let _claimed = claim_one(&store, &pred); // claimed, never acked → "crash"
            assert_eq!(store.read(&pred).len(), 0, "claimed in this run");
        }
        let store = BoardStore::persistent(&path, 1).unwrap();
        let recovered = store.read(&pred);
        assert_eq!(recovered.len(), 1, "the unacked claim re-queues as claimable");
        assert_eq!(recovered[0].payload.as_ref(), b"finite surplus");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn acked_fact_does_not_resurrect_on_replay() {
        let path = temp_wal("ack-replay");
        let pred = Predicate::new().eq("kind", "surplus");
        {
            let store = BoardStore::persistent(&path, 1).unwrap();
            store.post(surplus("4", "3.2"), Bytes::from("x")).unwrap();
            let claimed = claim_one(&store, &pred);
            store.ack(claimed.id).unwrap();
        }
        let store = BoardStore::persistent(&path, 1).unwrap();
        assert_eq!(store.read(&pred).len(), 0, "an acked fact is gone for good across replay");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn compaction_preserves_live_facts_and_drops_acked() {
        let path = temp_wal("compact");
        let pred = Predicate::new().eq("kind", "surplus");
        let store = BoardStore::persistent(&path, 1000).unwrap();
        // Post 4, ack 2 (claim+ack), leave 2 claimable.
        for i in 0..4 {
            store.post(surplus("4", &i.to_string()), Bytes::from(format!("p{i}"))).unwrap();
        }
        for _ in 0..2 {
            let c = claim_one(&store, &pred);
            store.ack(c.id).unwrap();
        }
        let epoch_before = store.wal_epoch();
        store.compact().unwrap();
        assert_eq!(store.wal_epoch(), epoch_before + 1, "compaction bumps the epoch");
        drop(store);

        let store = BoardStore::persistent(&path, 1000).unwrap();
        assert_eq!(store.read(&pred).len(), 2, "the 2 live facts survive compaction; the 2 acked are gone");
        let _ = std::fs::remove_file(&path);
    }

    /// Row C (post-360 hardening): a failed append leaves part of a frame on disk. The writer used
    /// to carry on, so the next acknowledged post landed *behind* the torn frame, and the next open
    /// read the torn frame's length over it, called the lot a torn tail and truncated it — an
    /// acknowledged post silently gone. Every post that returned `Ok` must survive a reopen.
    #[test]
    fn a_failed_append_never_strands_a_later_acknowledged_post() {
        let path = temp_wal("failed-append");
        let mut acked = Vec::new();
        {
            let store = BoardStore::persistent(&path, 1).unwrap();
            acked.push(store.post(surplus("1", "1.0"), Bytes::from("before")).unwrap());
            store.wal.as_ref().unwrap().fault.store(true, Ordering::SeqCst);
            assert!(store.post(surplus("2", "2.0"), Bytes::from(vec![7u8; 200])).is_err(), "the injected failure is reported");
            // After the failure, a post either is refused or survives the reopen — never both Ok and lost.
            if let Ok(id) = store.post(surplus("3", "3.0"), Bytes::from("after")) {
                acked.push(id);
            }
        }
        let store = BoardStore::persistent(&path, 1)
            .expect("a torn final frame is a crash's signature: truncated, not refused");
        let got: Vec<u64> = store.read(&Predicate::new()).iter().map(|f| f.id).collect();
        assert_eq!(got, acked, "every acknowledged post survives the reopen");
        let _ = std::fs::remove_file(&path);
    }

    /// Row C: a complete frame that does not decode, with records after it, is corruption — not a
    /// crash. The open used to stop there and truncate, discarding every later record silently. It
    /// must refuse by name and leave the file untouched.
    #[test]
    fn a_corrupt_middle_record_refuses_the_open_and_leaves_the_file() {
        let path = temp_wal("corrupt-middle");
        {
            let store = BoardStore::persistent(&path, 1).unwrap();
            store.post(surplus("1", "1.0"), Bytes::from("first")).unwrap();
        }
        let mut bytes = std::fs::read(&path).unwrap();
        // A whole frame of an unknown kind, then a good record after it.
        bytes.push(99);
        bytes.extend_from_slice(&8u32.to_le_bytes());
        bytes.extend_from_slice(&[0xAB; 8]);
        WalRecord::Post { id: 1, attributes: surplus("2", "2.0"), payload: Bytes::from("second") }.encode(&mut bytes);
        std::fs::write(&path, &bytes).unwrap();
        let err = match BoardStore::persistent(&path, 1) {
            Err(BlackboardError::Io(e)) => e,
            Err(other) => panic!("expected an io refusal, got {other}"),
            Ok(_) => panic!("a corrupt middle record refuses the open"),
        };
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains(&path.display().to_string()), "the refusal names the file: {err}");
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "a refused open leaves the file as it was");
        let _ = std::fs::remove_file(&path);
    }

    /// Row C: a write appends to the WAL and *then* applies to memory. A compaction that rewrote the
    /// log from memory, landing between the two, wrote a log without the record — and the post was
    /// still acknowledged, so a crash after that compaction lost it. The acknowledged post must
    /// survive a reopen whatever compaction ran in that window.
    #[test]
    fn a_compaction_between_append_and_apply_keeps_the_acknowledged_post() {
        let path = temp_wal("compact-window");
        let id = {
            let store = BoardStore::persistent(&path, 1).unwrap();
            COMPACT_BETWEEN.with(|c| c.set(true));
            let id = store.post(surplus("1", "1.0"), Bytes::from("in-the-window")).unwrap();
            assert!(!COMPACT_BETWEEN.with(|c| c.get()), "the compaction ran in the window");
            id
        };
        let store = BoardStore::persistent(&path, 1).unwrap();
        let got: Vec<u64> = store.read(&Predicate::new()).iter().map(|f| f.id).collect();
        assert_eq!(got, vec![id], "the acknowledged post survives the compaction and the reopen");
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 7: compaction drops acked facts, and open fenced `next_id` from the
    /// highest id *left*, so a restart reissued an acknowledged id.
    #[test]
    fn a_restart_never_reuses_an_acked_id() {
        let path = temp_wal("id-high-water");
        let first;
        {
            let store = BoardStore::persistent(&path, 1).unwrap();
            // Relative to the first id: a fresh WAL's id space does not start at 0 (ER3).
            first = store.post(surplus("0", "1.0"), Bytes::from("x")).unwrap();
            for i in 1..3 {
                store.post(surplus(&i.to_string(), "1.0"), Bytes::from("x")).unwrap();
            }
            while let Some(f) = store.claim(&Predicate::new()).unwrap() {
                store.ack(f.id).unwrap();
            }
            store.compact().unwrap();
        }
        let store = BoardStore::persistent(&path, 1).unwrap();
        assert_eq!(store.post(surplus("9", "1.0"), Bytes::from("next")).unwrap(), first + 3, "the next id is past every id the log held");
        let _ = std::fs::remove_file(&path);
    }

    /// ER3 (R4-01), the board's form: a fresh id space started at 0, so a transient board recreated
    /// in the same namespace reissued the ids of its previous life.
    #[test]
    fn fresh_id_spaces_do_not_overlap() {
        let first = BoardStore::transient();
        let old: Vec<u64> =
            (0..3).map(|i| first.post(surplus(&i.to_string(), "1.0"), Bytes::from("x")).unwrap()).collect();
        drop(first);
        let second = BoardStore::transient();
        let new = second.post(surplus("9", "1.0"), Bytes::from("new")).unwrap();
        assert!(new > *old.iter().max().unwrap(), "the restarted board reissued an id: old {old:?}, new {new}");
        // And a WAL with no history starts fresh too: a deleted WAL does not recycle ids.
        let path = temp_wal("fresh-after-transient");
        let walled = BoardStore::persistent(&path, 1).unwrap();
        let from_wal = walled.post(surplus("9", "1.0"), Bytes::from("wal")).unwrap();
        assert!(from_wal > new, "a WAL with no history reissued an id: {from_wal} after {new}");
        drop(walled);
        let _ = std::fs::remove_file(&path);
    }

    /// ER3: a WAL that names ids keeps its fence at exactly `max_id + 1`, whatever the clock says.
    /// A control: passes before and after the fix.
    #[test]
    fn a_wal_with_history_keeps_its_fence() {
        let path = temp_wal("history-fence");
        let last;
        {
            let store = BoardStore::persistent(&path, 1).unwrap();
            store.post(surplus("1", "1.0"), Bytes::from("a")).unwrap();
            last = store.post(surplus("2", "1.0"), Bytes::from("b")).unwrap();
        }
        let store = BoardStore::persistent(&path, 1).unwrap();
        assert_eq!(store.post(surplus("3", "1.0"), Bytes::from("c")).unwrap(), last + 1, "the fence is the log's");
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    /// ER3: ids stay below 2^53, exact as JSON numbers. A pin: an id space at 0 passes it too.
    #[test]
    fn ids_stay_below_two_to_the_53() {
        let id = BoardStore::transient().post(surplus("1", "1.0"), Bytes::from("x")).unwrap();
        assert!(id < 1u64 << 53, "id {id} is not exact as a JSON number");
    }

    /// ER3, the clock half (review of f4972912): every restart test above runs in one process, so the
    /// in-process mark alone passes them. A fresh store's first id is at or above the wall clock's
    /// base, so a new process — a fresh mark — cannot start below it.
    #[test]
    fn a_fresh_store_starts_at_or_above_the_wall_clock() {
        let before = mycelium::sim_seam::wall_now_ms() << 10;
        let id = BoardStore::transient().post(surplus("1", "1.0"), Bytes::from("x")).unwrap();
        assert!(id >= before, "the first id {id} is below the clock's base {before}");
    }

    /// ER3, the clock half: a restart in a **new process** — modelled by a mark of its own at 0, not
    /// by resetting the shared static, which other tests in this binary use concurrently — starts
    /// above the old store's ids once the clock has moved past them (the named limit: within
    /// *N*/1024 ms they can overlap).
    #[test]
    fn a_new_process_starts_above_the_old_stores_ids() {
        let old = BoardStore::transient();
        let old_ids: Vec<u64> = (0..3).map(|_| old.post(surplus("1", "1.0"), Bytes::from("x")).unwrap()).collect();
        let old_max = *old_ids.iter().max().unwrap();
        drop(old);
        // The old process's ids may run ahead of the clock by the in-process bump (one ms per store
        // created in this binary); wait for the clock to pass them, bounded by that bump.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while mycelium::sim_seam::wall_now_ms() << 10 <= old_max {
            assert!(std::time::Instant::now() < deadline, "the clock did not pass {old_max}");
            std::thread::yield_now();
        }
        let new_process = AtomicU64::new(0);
        let new = fresh_id_base_after(&new_process);
        assert!(new > old_max, "a new process reissued an id: old {old_ids:?}, new base {new}");
    }

    /// #597 review, finding 3: a valid kind byte followed by zeros is a zero-extended partial append —
    /// a torn tail, not corruption.
    #[test]
    fn a_valid_kind_followed_by_zeros_is_a_torn_tail() {
        let path = temp_wal("zero-tail");
        {
            let store = BoardStore::persistent(&path, 1).unwrap();
            store.post(surplus("1", "1.0"), Bytes::from("kept")).unwrap();
        }
        let mut bytes = std::fs::read(&path).unwrap();
        let good = bytes.len();
        bytes.push(1); // REC_POST
        bytes.extend_from_slice(&[0u8; 12]);
        std::fs::write(&path, &bytes).unwrap();
        let store = BoardStore::persistent(&path, 1).expect("a zero-filled tail is torn, not corrupt");
        assert_eq!(store.read(&Predicate::new()).len(), 1);
        assert_eq!(std::fs::metadata(&path).unwrap().len() as usize, good, "the torn tail is truncated");
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 6: a whole frame of a known kind whose body does not decode, with a record
    /// after it, refuses the open.
    #[test]
    fn a_known_kind_whose_body_does_not_decode_refuses_the_open() {
        let path = temp_wal("bad-body");
        {
            let store = BoardStore::persistent(&path, 1).unwrap();
            store.post(surplus("1", "1.0"), Bytes::from("first")).unwrap();
        }
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.push(1); // REC_POST
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&[1, 2, 3]);
        WalRecord::Post { id: 1, attributes: surplus("2", "2.0"), payload: Bytes::from("second") }.encode(&mut bytes);
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(BoardStore::persistent(&path, 1), Err(BlackboardError::Io(e)) if e.kind() == std::io::ErrorKind::InvalidData));
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 6: the repair path end to end.
    #[test]
    fn a_compaction_repairs_a_poisoned_wal_and_appends_resume() {
        let path = temp_wal("repair");
        let (a, c) = {
            let store = BoardStore::persistent(&path, 1).unwrap();
            let a = store.post(surplus("1", "1.0"), Bytes::from("a")).unwrap();
            store.inject_append_fault();
            assert!(store.post(surplus("2", "1.0"), Bytes::from("lost")).is_err());
            assert!(store.post(surplus("3", "1.0"), Bytes::from("refused")).is_err(), "poisoned");
            assert!(store.wants_compaction(), "a poisoned writer asks for the repair");
            store.compact().unwrap();
            assert!(!store.wal_poisoned());
            let c = store.post(surplus("4", "1.0"), Bytes::from("c")).unwrap();
            (a, c)
        };
        let store = BoardStore::persistent(&path, 1).unwrap();
        let got: Vec<u64> = store.read(&Predicate::new()).iter().map(|f| f.id).collect();
        assert_eq!(got, vec![a, c]);
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 6: an ack (memory first, then the WAL) racing a compaction is kept.
    #[test]
    fn an_ack_racing_a_compaction_is_kept() {
        let path = temp_wal("ack-window");
        {
            let store = BoardStore::persistent(&path, 1).unwrap();
            store.post(surplus("1", "1.0"), Bytes::from("x")).unwrap();
            let f = store.claim(&Predicate::new()).unwrap().unwrap();
            COMPACT_BETWEEN.with(|c| c.set(true));
            store.ack(f.id).unwrap();
            assert!(!COMPACT_BETWEEN.with(|c| c.get()), "the compaction ran in the window");
        }
        let store = BoardStore::persistent(&path, 1).unwrap();
        assert!(store.read(&Predicate::new()).is_empty(), "the acked fact does not come back");
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 1: a mirror's `discard` whose `Ack` record is refused is reported and not
    /// counted (pinned: this already held — the `?` returns before the count).
    #[test]
    fn a_refused_mirror_discard_is_not_counted() {
        let path = temp_wal("mirror-discard");
        let store = BoardStore::persistent(&path, 1).unwrap();
        store.post_with_id(5, surplus("1", "1.0"), Bytes::from("x")).unwrap();
        store.inject_append_fault();
        assert!(store.discard(5).is_err());
        assert_eq!(store.stats().acked, 0);
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 4: a repair that keeps failing reread the whole log on every tick. It
    /// backs off, and a later tick still repairs.
    #[test]
    fn a_failing_repair_backs_off() {
        let path = temp_wal("repair-backoff");
        let store = BoardStore::persistent(&path, 1).unwrap();
        store.inject_append_fault();
        assert!(store.post(surplus("1", "1.0"), Bytes::from("x")).is_err());
        store.set_compact_fault(true);
        let mut attempts = 0;
        for _ in 0..20 {
            if store.wants_compaction() {
                attempts += 1;
                assert!(store.compact().is_err());
            }
        }
        assert!(attempts <= 5, "{attempts} attempts in 20 ticks");
        store.set_compact_fault(false);
        let mut repaired = false;
        for _ in 0..200 {
            store.maintenance_tick();
            if !store.wal_poisoned() {
                repaired = true;
                break;
            }
        }
        assert!(repaired, "the back-off ends: a later tick repairs");
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 5: the compaction temp file was `with_extension("wal.compact")`, so `x.a`
    /// and `x.b` shared `x.wal.compact`, outside either's ownership lock.
    #[test]
    fn the_compaction_temp_file_is_named_after_the_whole_wal_file() {
        let dir = std::env::temp_dir().join(format!("mbb-stem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("x.wal.compact")).unwrap();
        let store = BoardStore::persistent(dir.join("x.a"), 1).unwrap();
        store.post(surplus("1", "1.0"), Bytes::from("x")).unwrap();
        store.compact().expect("the temp path is the WAL's own, not its stem's");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #597 re-review, finding 5: a compaction failing *after* its rename left the writer appending to
    /// the old, unlinked file, unpoisoned.
    #[test]
    fn a_failure_after_the_rename_poisons_the_writer() {
        let path = temp_wal("post-rename");
        let (a, c) = {
            let store = BoardStore::persistent(&path, 1).unwrap();
            let a = store.post(surplus("1", "1.0"), Bytes::from("a")).unwrap();
            store.set_post_rename_fault(true);
            assert!(store.compact().is_err());
            assert!(store.wal_poisoned(), "an error after the rename poisons the writer");
            assert!(store.post(surplus("2", "1.0"), Bytes::from("refused")).is_err());
            store.set_post_rename_fault(false);
            store.compact().unwrap();
            let c = store.post(surplus("3", "1.0"), Bytes::from("c")).unwrap();
            (a, c)
        };
        let store = BoardStore::persistent(&path, 1).unwrap();
        let got: Vec<u64> = store.read(&Predicate::new()).iter().map(|f| f.id).collect();
        assert_eq!(got, vec![a, c]);
        let _ = std::fs::remove_file(&path);
    }

    /// #597 re-review, finding 4: a compaction failing on an unpoisoned writer was retried every tick.
    #[test]
    fn a_failing_compaction_backs_off_even_when_not_poisoned() {
        let path = temp_wal("unpoisoned-backoff");
        let store = BoardStore::persistent(&path, 1).unwrap();
        for i in 0..64 {
            store.post(surplus(&i.to_string(), "1.0"), Bytes::from("x")).unwrap();
        }
        for _ in 0..32 {
            let f = store.claim(&Predicate::new()).unwrap().unwrap();
            store.ack(f.id).unwrap();
        }
        store.set_compact_fault(true);
        let mut attempts = 0;
        for _ in 0..20 {
            if store.wants_compaction() {
                attempts += 1;
                assert!(store.compact().is_err());
            }
        }
        assert!(attempts <= 5, "{attempts} attempts in 20 ticks");
        let _ = std::fs::remove_file(&path);
    }

    /// #597 re-review, finding 4: the board ticks once a second, so a 64-tick ceiling was a minute of
    /// refused appends. The ceiling is ~10 s.
    #[test]
    fn the_repair_back_off_is_capped_near_ten_seconds() {
        let path = temp_wal("backoff-cap");
        let store = BoardStore::persistent(&path, 1).unwrap();
        store.inject_append_fault();
        assert!(store.post(surplus("1", "1.0"), Bytes::from("x")).is_err());
        store.set_compact_fault(true);
        let mut last = None;
        let mut widest = 0;
        for tick in 0..300u32 {
            if store.wants_compaction() {
                let _ = store.compact();
                if let Some(prev) = last {
                    widest = widest.max(tick - prev);
                }
                last = Some(tick);
            }
        }
        assert!(widest <= 11, "{widest} ticks (seconds) between repair attempts");
        let _ = std::fs::remove_file(&path);
    }

    /// #597 re-review, finding 7: the id high-water mark across two compactions and restarts.
    #[test]
    fn the_high_water_mark_survives_two_compactions_and_a_restart() {
        let path = temp_wal("hw-twice");
        let first;
        {
            let store = BoardStore::persistent(&path, 1).unwrap();
            // Relative to the first id: a fresh WAL's id space does not start at 0 (ER3).
            first = store.post(surplus("0", "1.0"), Bytes::from("x")).unwrap();
            for i in 1..3 {
                store.post(surplus(&i.to_string(), "1.0"), Bytes::from("x")).unwrap();
            }
            while let Some(f) = store.claim(&Predicate::new()).unwrap() {
                store.ack(f.id).unwrap();
            }
            store.compact().unwrap();
        }
        {
            let store = BoardStore::persistent(&path, 1).unwrap();
            assert_eq!(store.post(surplus("y", "1.0"), Bytes::from("y")).unwrap(), first + 3);
            let f = store.claim(&Predicate::new()).unwrap().unwrap();
            store.ack(f.id).unwrap();
            store.compact().unwrap();
            store.compact().unwrap();
        }
        let store = BoardStore::persistent(&path, 1).unwrap();
        assert_eq!(store.post(surplus("z", "1.0"), Bytes::from("z")).unwrap(), first + 4, "past both compactions and restarts");
        let _ = std::fs::remove_file(&path);
    }

    /// Row C: one owner per WAL file. Two boards on one path used to replay, append and compact the
    /// same file, each over the other.
    #[test]
    fn a_second_owner_of_the_wal_is_refused() {
        let path = temp_wal("second-owner");
        let first = BoardStore::persistent(&path, 1).unwrap();
        match BoardStore::persistent(&path, 1) {
            Err(BlackboardError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::WouldBlock, "{e}"),
            Err(other) => panic!("expected an io refusal, got {other}"),
            Ok(_) => panic!("a second owner is refused"),
        }
        drop(first);
        BoardStore::persistent(&path, 1).expect("the lock is released with its owner");
        let _ = std::fs::remove_file(&path);
    }

    /// Row C: compaction installs its file the way the core installs a snapshot — the temp file's
    /// bytes synced, renamed into place, then the **directory** synced, without which the rename
    /// (and so the compaction) may not survive a power loss.
    #[test]
    fn compaction_syncs_the_temp_file_and_the_directory() {
        let path = temp_wal("compact-durable");
        let store = BoardStore::persistent(&path, 1).unwrap();
        store.post(surplus("1", "1.0"), Bytes::from("x")).unwrap();
        crate::wal::FS_TRACE.with(|t| t.borrow_mut().clear());
        store.compact().unwrap();
        let trace = crate::wal::FS_TRACE.with(|t| t.borrow().clone());
        let pos = |op: &str| trace.iter().position(|o| *o == op).unwrap_or_else(|| panic!("no {op} in {trace:?}"));
        assert!(pos("tmp.write") < pos("tmp.sync"), "{trace:?}");
        assert!(pos("tmp.sync") < pos("rename"), "{trace:?}");
        assert!(pos("rename") < pos("dir.sync"), "the directory sync makes the rename durable: {trace:?}");
        let _ = std::fs::remove_file(&path);
    }

    /// #718's review, finding 3: reopening an existing WAL syncs its directory again, as the
    /// journal does. On the code before the fix a reopen syncs nothing.
    #[test]
    fn reopening_a_wal_syncs_its_directory() {
        let tag = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("mbb-r6-reopen-{}-{tag}", std::process::id()));
        let path = root.join("a").join("wal.log");
        drop(BoardStore::persistent(&path, 1).unwrap());
        crate::wal::FS_DIRS.with(|t| t.borrow_mut().clear());
        let store = BoardStore::persistent(&path, 1).unwrap();
        let dirs = crate::wal::FS_DIRS.with(|t| t.borrow().clone());
        let under: Vec<_> = dirs.iter().filter(|(_, p)| p.starts_with(&root)).cloned().collect();
        assert_eq!(
            under,
            [("open.mkdir.sync", root.clone()), ("open.parent.sync", root.join("a"))],
            "`a`'s entry in the root, then the file's in `a`, on a reopen: {dirs:?}"
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// R6 of `docs/plans/external-review-2026-10-31.md` (the review's F7): a fresh WAL's directory
    /// entry is synced before its first record is synced, and each directory `open` creates is
    /// synced in the one above it. On the unrepaired code neither sync runs. The power-loss
    /// property itself is not observable without a filesystem fault adapter; this pins the order.
    #[test]
    fn a_fresh_wal_syncs_its_directories_before_the_first_record() {
        let tag = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("mbb-r6-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("a").join("wal.log");
        crate::wal::FS_TRACE.with(|t| t.borrow_mut().clear());
        let store = BoardStore::persistent(&path, 1).unwrap();
        store.post(surplus("1", "1.0"), Bytes::from("x")).unwrap();
        let trace = crate::wal::FS_TRACE.with(|t| t.borrow().clone());
        let opens: Vec<_> = trace.iter().filter(|o| **o != "append.sync").copied().collect();
        let dirs = opens.iter().filter(|o| **o == "open.mkdir.sync").count();
        assert!(dirs >= 2, "the root and `a` at least: {trace:?}");
        assert_eq!(&opens[dirs..], ["open.parent.sync"], "every directory, then the file's entry in `a`: {trace:?}");
        let parent = trace.iter().position(|o| *o == "open.parent.sync").unwrap();
        let append = trace.iter().position(|o| *o == "append.sync").unwrap_or_else(|| panic!("no append.sync in {trace:?}"));
        assert!(parent < append, "{trace:?}");
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_a_newer_wal_format() {
        // A future-version header must be refused, not silently truncated.
        let path = temp_wal("future-version");
        let mut bytes = b"MBBWAL".to_vec();
        bytes.extend_from_slice(&999u16.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        assert!(BoardStore::persistent(&path, 1).is_err());
        let _ = std::fs::remove_file(&path);
    }

    // ── ER6: the acked-id memory and the id fence ──

    /// ER6 (R11-01): the acked ids a board remembers survive a reopen and a compaction — those `ack`
    /// recorded and those `discard` recorded for a fact it never held — and a live fact stays live.
    #[test]
    fn tombstones_survive_reopen_and_compaction() {
        let path = temp_wal("tombstones-reopen");
        let (done, live, foreign) = {
            let store = BoardStore::persistent(&path, 1).unwrap();
            let done = store.post(surplus("1", "1.0"), Bytes::from("done")).unwrap();
            let live = store.post(surplus("2", "1.0"), Bytes::from("live")).unwrap();
            assert_eq!(claim_one(&store, &Predicate::new().eq("feeder", "1")).id, done);
            store.ack(done).unwrap();
            let foreign = live + 1_000;
            assert!(!store.discard(foreign).unwrap(), "absent, and remembered anyway");
            assert!(store.remembers_acked(done) && store.remembers_acked(foreign));
            (done, live, foreign)
        };
        for round in ["reopen", "compaction"] {
            let store = BoardStore::persistent(&path, 1).unwrap();
            assert!(store.remembers_acked(done), "{round}: the acked id is remembered");
            assert!(store.remembers_acked(foreign), "{round}: the discarded id is remembered");
            assert!(!store.remembers_acked(live), "{round}: a live id is not a tombstone");
            let ids: Vec<u64> = store.read(&Predicate::new()).iter().map(|f| f.id).collect();
            assert_eq!(ids, vec![live], "{round}: the live fact stays live");
            store.post_with_id(done, surplus("1", "1.0"), Bytes::from("back")).unwrap();
            assert_eq!(store.refused_acked(), 1, "{round}: a remembered id is refused");
            assert_eq!(store.read(&Predicate::new()).len(), 1);
            store.compact().unwrap();
        }
        let _ = std::fs::remove_file(&path);
    }

    /// ER6 (correction 3): a WAL an older build wrote can re-post an id after its `Ack`. Replay holds
    /// it live; the fold does not keep it a tombstone, so a compaction (tombstones before `Post`s)
    /// cannot kill it either.
    #[test]
    fn a_re_posted_id_stays_live_through_compaction() {
        let path = temp_wal("repost");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MBBWAL");
        bytes.extend_from_slice(&1u16.to_le_bytes());
        for rec in [
            WalRecord::Post { id: 5, attributes: surplus("1", "1.0"), payload: Bytes::from("a") },
            WalRecord::Ack { id: 5 },
            WalRecord::Post { id: 5, attributes: surplus("1", "1.0"), payload: Bytes::from("b") },
            WalRecord::Ack { id: 6 },
        ] {
            rec.encode(&mut bytes);
        }
        std::fs::write(&path, &bytes).unwrap();
        for round in ["open", "after compaction"] {
            let store = BoardStore::persistent(&path, 1).unwrap();
            let ids: Vec<u64> = store.read(&Predicate::new()).iter().map(|f| f.id).collect();
            assert_eq!(ids, vec![5], "{round}");
            assert!(!store.remembers_acked(5) && store.remembers_acked(6), "{round}");
            store.compact().unwrap();
        }
        let _ = std::fs::remove_file(&path);
    }

    /// ER6 (R11-04, D6d): `fence_past` moves the id fence and persists it as a lone `Claim`, which
    /// survives a compaction and a reopen *as a fence and not as a tombstone*; an `n` below the
    /// board's own base (ER3) changes nothing and logs nothing.
    #[test]
    fn fence_past_persists_as_a_claim_not_a_tombstone() {
        let path = temp_wal("fence");
        let (first, fence) = {
            let store = BoardStore::persistent(&path, 1).unwrap();
            let first = store.post(surplus("1", "1.0"), Bytes::from("x")).unwrap();
            let len = std::fs::metadata(&path).unwrap().len();
            assert!(!store.fence_past(first).unwrap(), "below the next id: no move");
            assert!(!store.fence_past(1).unwrap(), "below the board's own base: no move");
            assert_eq!(std::fs::metadata(&path).unwrap().len(), len, "and nothing logged");
            let fence = first + 50;
            assert!(store.fence_past(fence).unwrap());
            assert!(!store.remembers_acked(fence - 1));
            (first, fence)
        };
        for round in ["reopen", "compaction"] {
            let store = BoardStore::persistent(&path, 1).unwrap();
            assert!(!store.remembers_acked(fence - 1), "{round}: a fence is not a tombstone");
            assert_eq!(store.read(&Predicate::new()).len(), 1, "{round}: the live fact {first} stays");
            store.compact().unwrap();
        }
        let store = BoardStore::persistent(&path, 1).unwrap();
        let next = store.post(surplus("2", "1.0"), Bytes::new()).unwrap();
        assert!(next >= fence, "the reopened board issued {next}, below its fence {fence}");
        // A replicated fact under the fence id after all: it applies, and stays live across reopen.
        store.post_with_id(fence - 1, surplus("3", "1.0"), Bytes::new()).unwrap();
        drop(store);
        let store = BoardStore::persistent(&path, 1).unwrap();
        assert_eq!(store.read(&Predicate::new().eq("feeder", "3")).len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    /// ER6: a replicated post of an id this board holds is a no-op — one fact, one `Post` in the log.
    #[test]
    fn a_re_delivered_post_is_applied_once() {
        let path = temp_wal("redelivered");
        let store = BoardStore::persistent(&path, 1).unwrap();
        store.post_with_id(9, surplus("1", "1.0"), Bytes::from("x")).unwrap();
        let len = std::fs::metadata(&path).unwrap().len();
        store.post_with_id(9, surplus("1", "1.0"), Bytes::from("x")).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), len);
        assert_eq!(store.stats().posted, 1);
        let _ = std::fs::remove_file(&path);
    }

    /// ER6 (correction 6): a tombstone is logged once — a re-applied known tombstone writes nothing.
    #[test]
    fn a_known_tombstone_is_logged_once() {
        let path = temp_wal("tombstone-once");
        let store = BoardStore::persistent(&path, 1).unwrap();
        store.discard(77).unwrap();
        let len = std::fs::metadata(&path).unwrap().len();
        for _ in 0..3 {
            store.discard(77).unwrap();
        }
        assert_eq!(std::fs::metadata(&path).unwrap().len(), len);
        assert_eq!(store.stats().acked, 0, "no fact was removed");
        let _ = std::fs::remove_file(&path);
    }

    /// Review N7: a compaction keeps exactly the newest `COMPACTION_TOMBSTONES` and forgets the
    /// older ones at the reopen.
    #[test]
    fn compaction_keeps_exactly_the_newest_tombstones() {
        use crate::wal::COMPACTION_TOMBSTONES;
        let path = temp_wal("newest-tombstones");
        let extra = 10u64;
        let first = {
            let store = BoardStore::persistent(&path, 100_000).unwrap();
            let first = store.post(surplus("1", "1.0"), Bytes::new()).unwrap() + 1;
            for id in first..first + COMPACTION_TOMBSTONES as u64 + extra {
                store.discard(id).unwrap();
            }
            store.compact().unwrap();
            first
        };
        let store = BoardStore::persistent(&path, 1).unwrap();
        assert_eq!(store.acked_memory_len(), COMPACTION_TOMBSTONES);
        for id in first..first + extra {
            assert!(!store.remembers_acked(id), "{id}: older than the kept tombstones");
        }
        assert!(store.remembers_acked(first + extra));
        assert!(store.remembers_acked(first + COMPACTION_TOMBSTONES as u64 + extra - 1));
        let _ = std::fs::remove_file(&path);
    }
}
