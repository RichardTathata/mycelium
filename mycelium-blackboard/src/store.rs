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

use crate::wal::{WalRecord, WalWriter};

#[cfg(test)]
thread_local! {
    static COMPACT_BETWEEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
use crate::{BlackboardError, Fact, Predicate};

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
        let (wal, live, max_id) = WalWriter::open(path.as_ref(), checkpoint_every)?;
        let store = Self::with_wal(Some(wal));
        store.next_id.store(max_id.map_or(0, |m| m + 1), Ordering::Relaxed);
        {
            let mut g = store.inner.lock();
            for (id, attributes, payload) in live {
                g.available.insert(id, Fact { id, attributes, payload });
                store.posted.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(store)
    }

    fn with_wal(wal: Option<WalWriter>) -> Self {
        Self {
            inner: Mutex::new(BoardInner { available: BTreeMap::new(), inflight: HashMap::new() }),
            next_id: AtomicU64::new(0),
            posted: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            high_watermark: None,
            claimed: AtomicU64::new(0),
            acked: AtomicU64::new(0),
            released: AtomicU64::new(0),
            requeued: AtomicU64::new(0),
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

    /// Apply a fact under its ORIGINAL id (replication / WAL replay — later phases), fencing
    /// `next_id` past it so a promoted secondary never re-issues a live id.
    pub fn post_with_id(&self, id: u64, attributes: BTreeMap<String, String>, payload: Bytes) -> Result<(), BlackboardError> {
        self.next_id.fetch_max(id + 1, Ordering::Relaxed);
        if let Some(wal) = &self.wal {
            wal.append(&WalRecord::Post { id, attributes: attributes.clone(), payload: payload.clone() })?;
        }
        self.inner.lock().available.insert(id, Fact { id, attributes, payload });
        self.posted.fetch_add(1, Ordering::Relaxed);
        Ok(())
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
        let removed = self.inner.lock().inflight.remove(&id).is_some();
        if !removed {
            return Err(BlackboardError::NotFound);
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
    #[cfg(test)]
    pub(crate) fn wal_poisoned(&self) -> bool {
        self.wal.as_ref().unwrap().poisoned()
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

    /// Snapshot of every live fact (claimable + in-flight) — the secondary's initial sync source.
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
    pub fn discard(&self, id: u64) -> Result<bool, BlackboardError> {
        let removed = {
            let mut g = self.inner.lock();
            g.available.remove(&id).is_some() || g.inflight.remove(&id).is_some()
        };
        if removed {
            if let Some(wal) = &self.wal {
                wal.append(&WalRecord::Ack { id })?;
            }
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
        {
            let store = BoardStore::persistent(&path, 1).unwrap();
            store.post(surplus("4", "1.0"), Bytes::from("a")).unwrap();
            store.post(surplus("4", "2.0"), Bytes::from("b")).unwrap();
        }
        // Reopen — both facts replay as claimable, ids fenced.
        let store = BoardStore::persistent(&path, 1).unwrap();
        assert_eq!(store.read(&pred).len(), 2);
        let fresh = store.post(surplus("4", "3.0"), Bytes::from("c")).unwrap();
        assert!(fresh >= 2, "next_id fenced past replayed ids");
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
        {
            let store = BoardStore::persistent(&path, 1).unwrap();
            for i in 0..3 {
                store.post(surplus(&i.to_string(), "1.0"), Bytes::from("x")).unwrap();
            }
            while let Some(f) = store.claim(&Predicate::new()).unwrap() {
                store.ack(f.id).unwrap();
            }
            store.compact().unwrap();
        }
        let store = BoardStore::persistent(&path, 1).unwrap();
        assert_eq!(store.post(surplus("9", "1.0"), Bytes::from("next")).unwrap(), 3, "the next id is past every id the log held");
        let _ = std::fs::remove_file(&path);
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
}
