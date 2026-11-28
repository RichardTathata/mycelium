//! Local execution substrate: per-stage queues, parked waiters, in-flight
//! tracking, and the optional write-ahead log.
//!
//! This module has no dependency on the Mycelium agent — it is the same kind
//! of node-local working memory as the `DashMap` inside the KV store. All
//! cluster concerns (RPC, capability election, inflight visibility keys) live
//! in `rpc.rs` / `lib.rs`.
//!
//! ## Lock order
//!
//! Three lock families exist here. When held simultaneously they must be
//! acquired in this order (release in reverse):
//!
//! 1. `WalInner` — held across the whole compaction rewrite; appends are
//!    short. Compaction folds the log itself under it and takes no other lock
//!    (row C: a snapshot of memory could miss a record appended but not yet
//!    applied), so today no path nests the WAL lock with another.
//! 2. `StageInner` — the put/take hot-path lock; never held across `await`.
//! 3. `inflight` — leaf lock. `dispatch` and `take` acquire it while holding
//!    a `StageInner` lock (stage → inflight), never the reverse.
//!
//! No path holds a stage or inflight lock while *waiting* for the WAL lock
//! (appends happen after the stage guard is dropped), and compaction no longer
//! takes a stage or inflight lock inside the WAL lock, so the two families
//! never nest.

use bytes::Bytes;
use parking_lot::Mutex;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;

use crate::TupleError;

// ─── Stage state ─────────────────────────────────────────────────────────────

/// Inner state under a single lock — eliminates TOCTOU between waiter-check
/// and entry-store. The `Instant` is the enqueue time, for store-path queue
/// wait measurement (hot-path items never carry one — they never queue).
struct StageInner {
    entries: VecDeque<(u64, Bytes, std::time::Instant)>,
    waiters: VecDeque<oneshot::Sender<(u64, Bytes)>>,
    /// M13 (WS-G / G1a) keyed-exact-match rendezvous, kept **separate** from the FIFO so a keyed
    /// `take_by_key` and an unkeyed `take` on the same stage never interfere. Items put with a
    /// correlation key wait here to be claimed by that exact key (an O(1) hash lookup — not template
    /// matching). Both count toward `depth` (backpressure sees all queued items).
    ///
    /// **A FIFO queue per key** (ER7): items under one key are claimed in arrival order, a re-queued
    /// item at the back, as in the FIFO lane. A key is not unique — two outputs under one correlation
    /// are both kept. The map entry is removed when its queue empties. The order is this process's:
    /// a requeue writes no WAL record, so after a restart or a failover (WAL replay, `state_chunk`)
    /// a key's items are in id order.
    keyed_entries: HashMap<Arc<str>, VecDeque<(u64, Bytes, std::time::Instant)>>,
    /// Workers parked on a specific key, served in arrival order (ER7: a second waiter on a key no
    /// longer evicts the first). A timed-out or dropped take's registration is pruned by its
    /// `KeyedWait` guard (and on the key's next registration); the entry is removed when empty.
    keyed_waiters: HashMap<Arc<str>, VecDeque<oneshot::Sender<(u64, Bytes)>>>,
}

/// Ring of the last N store-path queue waits (µs), for P99 reporting.
/// Hot-path deliveries are deliberately excluded — including their
/// sub-microsecond times would collapse the distribution and hide true
/// queueing latency.
const QUEUE_WAIT_SAMPLES: usize = 1000;

pub(crate) struct StageState {
    inner: Mutex<StageInner>,
    /// Shadow counter for lock-free monitoring reads.
    pub(crate) depth: AtomicU32,
    /// Strict mirror of `waiters.len()` plus every keyed waiter queue's length
    /// (ER7: a worker parked on `take_by_key` is a waiting worker too):
    /// incremented at push, decremented at pop or prune, ALL under the stage
    /// lock. The plain timeout path deliberately does not touch it — the dead
    /// sender is still in the deque until a dispatch skips it, so the count
    /// may transiently include timed-out waiters but can never underflow. A
    /// keyed waiter's registration is removed however its take ends — timed
    /// out or dropped — by `KeyedWait`'s drop, so keyed waiters are exact. (Run-17 probe `metrics_accounting_identity` caught the
    /// previous double-decrement wrapping this to ~4.29e9.)
    pub(crate) waiters_count: AtomicU32,
    pub(crate) put_total: AtomicU64,
    pub(crate) take_total: AtomicU64,
    /// Puts delivered directly to a parked worker (never queued).
    pub(crate) hot_total: AtomicU64,
    /// Puts **refused at admission** because the stage stood at its high watermark (item 4 PR 5:
    /// a rejection is a visible outcome, reported beside the admitted and the taken, never a
    /// silence). Counted here, at the primary, which owns this deficit.
    pub(crate) rejected_total: AtomicU64,
    queue_waits_us: Mutex<VecDeque<u32>>,
}

impl StageState {
    fn new() -> Self {
        Self {
            inner: Mutex::new(StageInner {
                entries: VecDeque::new(),
                waiters: VecDeque::new(),
                keyed_entries: HashMap::new(),
                keyed_waiters: HashMap::new(),
            }),
            depth: AtomicU32::new(0),
            waiters_count: AtomicU32::new(0),
            put_total: AtomicU64::new(0),
            take_total: AtomicU64::new(0),
            hot_total: AtomicU64::new(0),
            rejected_total: AtomicU64::new(0),
            queue_waits_us: Mutex::new(VecDeque::with_capacity(64)),
        }
    }

    fn record_queue_wait(&self, waited: Duration) {
        let us = waited.as_micros().min(u32::MAX as u128) as u32;
        let mut g = self.queue_waits_us.lock();
        if g.len() == QUEUE_WAIT_SAMPLES {
            g.pop_front();
        }
        g.push_back(us);
    }

    /// P99 of the sampled store-path queue waits; 0 with no samples.
    pub(crate) fn queue_p99_us(&self) -> u32 {
        let mut samples: Vec<u32> = self.queue_waits_us.lock().iter().copied().collect();
        if samples.is_empty() {
            return 0;
        }
        samples.sort_unstable();
        samples[(samples.len() - 1) * 99 / 100]
    }
}

#[derive(Clone)]
pub(crate) struct Inflight {
    pub(crate) stage: Arc<str>,
    pub(crate) payload: Bytes,
    pub(crate) taken_at_ms: u64,
    /// M13 / WS-G: the correlation key if this item was put keyed, so a crash-requeue or compaction
    /// snapshot re-queues it under its key (not the FIFO). `None` for ordinary FIFO items.
    pub(crate) key: Option<Arc<str>>,
}

/// A parked `take_by_key`'s registration (ER7). Dropped however the take ends: while armed it
/// closes the receiver and, under the stage lock, prunes the key's closed senders and removes an
/// empty entry, keeping `waiters_count` exact — so a dropped future on a key nobody touches again
/// leaves nothing. Disarmed when a dispatcher has taken the sender (an item was received). The
/// stage lock is a synchronous `parking_lot` lock and the prune does no I/O, so this is safe in
/// `Drop`; nothing drops a take future while holding a stage lock.
struct KeyedWait {
    state: Arc<StageState>,
    key: Arc<str>,
    rx: oneshot::Receiver<(u64, Bytes)>,
    armed: bool,
}

impl Drop for KeyedWait {
    fn drop(&mut self) {
        if self.armed {
            // Close first, so our sender reads as closed to the prune (the receiver is still alive).
            // An item a dispatcher sent before the close stays in flight and re-queues at its lease.
            self.rx.close();
            TupleStore::prune_keyed_waiters(&self.state, &mut self.state.inner.lock(), &self.key);
        }
    }
}

// ─── TupleStore ──────────────────────────────────────────────────────────────

pub(crate) struct TupleStore {
    stages: papaya::HashMap<Arc<str>, Arc<StageState>>,
    /// Items taken (or hot-handed) but not yet acked. Source of truth for
    /// re-queue; the WAL mirrors it for crash recovery.
    inflight: Mutex<HashMap<u64, Inflight>>,
    next_id: AtomicU64,
    high_watermark: u32,
    wal: Option<WalWriter>,
}

pub(crate) struct StageDepth {
    pub stage: Arc<str>,
    pub depth: u32,
    pub waiters: u32,
}

pub(crate) struct StageMetrics {
    pub stage: Arc<str>,
    pub depth: u32,
    pub waiters: u32,
    pub inflight: u32,
    pub put_total: u64,
    pub take_total: u64,
    pub hot_total: u64,
    pub rejected_total: u64,
    pub queue_p99_us: u32,
}

impl TupleStore {
    /// Transient store (no WAL).
    pub(crate) fn transient(high_watermark: u32) -> Self {
        Self {
            stages: papaya::HashMap::new(),
            inflight: Mutex::new(HashMap::new()),
            // No id history: start past every id an earlier life could have issued (ER3).
            next_id: AtomicU64::new(fresh_id_base()),
            high_watermark,
            wal: None,
        }
    }

    /// WAL-backed store. Replays any existing log at `path`: every item with
    /// a `PutRecord`/`CompleteRecord` and no terminal ack is re-queued
    /// (including items that had a `TakeRecord` — they re-queue as abandoned).
    pub(crate) fn persistent(
        path: &Path,
        checkpoint_every: u64,
        high_watermark: u32,
    ) -> io::Result<Self> {
        let (wal, live, max_id) = WalWriter::open(path, checkpoint_every)?;
        let store = Self {
            stages: papaya::HashMap::new(),
            inflight: Mutex::new(HashMap::new()),
            // A WAL that names ids fences at its highest + 1; one that names none has no history,
            // so it starts fresh rather than at 0 (a deleted WAL must not recycle ids, ER3).
            next_id: AtomicU64::new(max_id.map_or_else(fresh_id_base, |m| m + 1)),
            high_watermark,
            wal: Some(wal),
        };
        for (id, stage, payload, key) in live {
            let state = store.stage(&stage);
            // Direct enqueue: no waiters can exist yet, no WAL write needed
            // (the records being replayed are already in the log). A keyed item re-queues into the
            // keyed index under its key (M13) so a post-restart take_by_key still rendezvouses.
            {
                let mut g = state.inner.lock();
                match key {
                    Some(k) => g.keyed_entries.entry(k).or_default().push_back((id, payload, std::time::Instant::now())),
                    None => g.entries.push_back((id, payload, std::time::Instant::now())),
                }
            }
            state.depth.fetch_add(1, Ordering::Relaxed);
        }
        Ok(store)
    }

    /// Test seam: the window between a write's two steps — its WAL append and its in-memory apply,
    /// in whichever order the write takes them. A test arms `COMPACT_BETWEEN` to run a compaction
    /// exactly there, on this thread.
    #[cfg(test)]
    fn between_append_and_apply(&self) {
        if COMPACT_BETWEEN.with(|c| c.replace(false)) {
            self.compact_now().expect("the interleaved compaction");
        }
    }

    fn stage(&self, name: &str) -> Arc<StageState> {
        let map = self.stages.pin();
        if let Some(s) = map.get(name) {
            return Arc::clone(s);
        }
        let key: Arc<str> = Arc::from(name);
        Arc::clone(map.get_or_insert_with(key, || Arc::new(StageState::new())))
    }

    /// Write item to `stage`. Returns the assigned item id.
    pub(crate) fn put(&self, stage: &str, payload: Bytes) -> Result<u64, TupleError> {
        let state = self.stage(stage);
        if state.depth.load(Ordering::Relaxed) >= self.high_watermark {
            state.rejected_total.fetch_add(1, Ordering::Relaxed);
            return Err(TupleError::Backpressure { retry_after_ms: 500 });
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // WAL once, before any dispatch decision — covers hot path and store
        // path with a single PutRecord regardless of how many dead waiters
        // the dispatch loop skips.
        if let Some(wal) = &self.wal {
            wal.append(&Record::Put {
                id,
                stage: Arc::from(stage),
                payload: payload.clone(),
                key: None,
            })?;
        }
        #[cfg(test)]
        self.between_append_and_apply();
        state.put_total.fetch_add(1, Ordering::Relaxed);
        let hot = self.dispatch(&state, Arc::from(stage), id, payload)?;
        if hot {
            // Release: the dispatch's `take_total` increment for this hand-off precedes this one in
            // program order, so a snapshot that Acquire-loads `hot_total` first sees it.
            state.hot_total.fetch_add(1, Ordering::Release);
        }
        Ok(id)
    }

    /// Hand `(id, payload)` to a parked waiter, or queue it. Returns `true`
    /// when the item was delivered directly (hot path).
    ///
    /// The item is registered in `inflight` *before* the oneshot send so a
    /// concurrent compaction snapshot can never observe it in neither place.
    fn dispatch(
        &self,
        state: &StageState,
        stage: Arc<str>,
        id: u64,
        mut payload: Bytes,
    ) -> Result<bool, TupleError> {
        let mut g = state.inner.lock();
        let mut undone = false;
        loop {
            match g.waiters.pop_front() {
                None => {
                    // After an undone hand-off (a failed send), at the front: an item queued while the
                    // lock was released for the send is usually newer, so a single undo keeps arrival
                    // order. **Limit:** two producers undoing at once each push to the front, so
                    // concurrent undone hand-offs may come back reversed relative to each other.
                    // Its queue wait is measured from here, not from its put.
                    let item = (id, payload, std::time::Instant::now());
                    if undone { g.entries.push_front(item) } else { g.entries.push_back(item) }
                    state.depth.fetch_add(1, Ordering::Relaxed);
                    return Ok(false);
                }
                Some(tx) => {
                    state.waiters_count.fetch_sub(1, Ordering::Relaxed);
                    self.inflight.lock().insert(
                        id,
                        Inflight {
                            stage: Arc::clone(&stage),
                            payload: payload.clone(),
                            taken_at_ms: now_ms(),
                            key: None,
                        },
                    );
                    drop(g); // release before send — no I/O or wakeup under lock
                    match tx.send((id, payload)) {
                        Ok(()) => {
                            // One take per hand-off, counted where the item enters in flight (ER7,
                            // R11-10) — the woken taker does not count it again.
                            state.take_total.fetch_add(1, Ordering::Relaxed);
                            if let Some(wal) = &self.wal {
                                wal.append(&Record::Take { id, taken_at_ms: now_ms() })?;
                            }
                            return Ok(true);
                        }
                        Err(p) => {
                            // Receiver timed out — undo, try next waiter or store.
                            self.inflight.lock().remove(&id);
                            payload = p.1;
                            undone = true;
                            g = state.inner.lock();
                        }
                    }
                }
            }
        }
    }

    /// Blocking claim. Parks until an item is available or `timeout` elapses.
    pub(crate) async fn take(
        &self,
        stage: &str,
        timeout: Duration,
    ) -> Result<(u64, Bytes), TupleError> {
        let state = self.stage(stage);
        let mut rx = {
            let mut g = state.inner.lock();
            if let Some((id, payload, enqueued)) = g.entries.pop_front() {
                state.depth.fetch_sub(1, Ordering::Relaxed);
                state.record_queue_wait(enqueued.elapsed());
                self.inflight.lock().insert(
                    id,
                    Inflight {
                        stage: Arc::from(stage),
                        payload: payload.clone(),
                        taken_at_ms: now_ms(),
                        key: None,
                    },
                );
                drop(g);
                if let Some(wal) = &self.wal {
                    wal.append(&Record::Take { id, taken_at_ms: now_ms() })?;
                }
                // Counted once the take is logged: a refused append returns `Err`, not the item.
                state.take_total.fetch_add(1, Ordering::Relaxed);
                return Ok((id, payload));
            }
            let (tx, rx) = oneshot::channel();
            g.waiters.push_back(tx);
            state.waiters_count.fetch_add(1, Ordering::Relaxed);
            rx
        }; // stage lock released before parking
        tokio::select! {
            // The dispatcher counted the hand-off in `take_total` when it sent.
            item = &mut rx => item.map_err(|_| TupleError::Timeout), // sender dropped: store shutdown
            _ = tokio::time::sleep(timeout) => {
                // Close first, so a dispatcher that has popped our sender but not yet sent
                // fails its send and keeps the item (ER7): without the close, a send after
                // the `try_recv` below would succeed into a receiver about to be dropped, and
                // the item would sit in flight with no owner until its lease expired. Then
                // drain: a send that landed before the close is ours (already in flight).
                rx.close();
                // No waiters_count decrement here: the dead sender is still queued; the
                // dispatch that eventually pops it performs the matching decrement.
                rx.try_recv().map_err(|_| TupleError::Timeout)
            }
        }
    }

    // ── M13 / G1a: keyed-exact-match rendezvous (fan-in joins) ───────────────

    /// Put `payload` on `stage` under correlation `key` (WS-G / M13). Claimed only by
    /// [`take_by_key`](Self::take_by_key) with the same key — the two-stream rendezvous ("an invoice
    /// AND its matching purchase order") that exact lane names can't express without one lane per
    /// key. Hands off to a parked keyed waiter (hot path) or stores under the key. Items under one
    /// key queue in arrival order and same-key waiters are served in arrival order (ER7).
    pub(crate) fn put_keyed(&self, stage: &str, key: Arc<str>, payload: Bytes) -> Result<u64, TupleError> {
        let state = self.stage(stage);
        if state.depth.load(Ordering::Relaxed) >= self.high_watermark {
            state.rejected_total.fetch_add(1, Ordering::Relaxed);
            return Err(TupleError::Backpressure { retry_after_ms: 500 });
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Some(wal) = &self.wal {
            // WAL v2: the key is persisted so a keyed in-flight item re-queues under its key across
            // a crash / promotion (G1b).
            wal.append(&Record::Put {
                id,
                stage: Arc::from(stage),
                payload: payload.clone(),
                key: Some(Arc::clone(&key)),
            })?;
        }
        state.put_total.fetch_add(1, Ordering::Relaxed);
        let hot = self.dispatch_keyed(&state, Arc::from(stage), key, id, payload)?;
        if hot {
            // Release: the dispatch's `take_total` increment for this hand-off precedes this one in
            // program order, so a snapshot that Acquire-loads `hot_total` first sees it.
            state.hot_total.fetch_add(1, Ordering::Release);
        }
        Ok(id)
    }

    /// Hand a keyed `(id, payload)` to the oldest waiter parked on `key`, or push it at the back of
    /// `key`'s queue. Returns `true` on a direct (hot-path) delivery. The same loop as
    /// [`dispatch`](Self::dispatch): the item is registered in `inflight` before the oneshot send so a
    /// compaction snapshot never loses it; a failed send (a timed-out or dropped taker) is undone and
    /// the next waiter tried.
    fn dispatch_keyed(
        &self,
        state: &StageState,
        stage: Arc<str>,
        key: Arc<str>,
        id: u64,
        mut payload: Bytes,
    ) -> Result<bool, TupleError> {
        let mut g = state.inner.lock();
        let mut undone = false;
        loop {
            let next = match g.keyed_waiters.get_mut(&key) {
                Some(q) => {
                    let tx = q.pop_front();
                    if q.is_empty() {
                        g.keyed_waiters.remove(&key);
                    }
                    tx
                }
                None => None,
            };
            match next {
                None => {
                    // After an undone hand-off, at the front of the key's queue — with `dispatch`'s
                    // limit: concurrent undone hand-offs may come back reversed relative to each other.
                    let item = (id, payload, std::time::Instant::now());
                    let q = g.keyed_entries.entry(key).or_default();
                    if undone { q.push_front(item) } else { q.push_back(item) }
                    state.depth.fetch_add(1, Ordering::Relaxed);
                    return Ok(false);
                }
                Some(tx) => {
                    state.waiters_count.fetch_sub(1, Ordering::Relaxed);
                    self.inflight.lock().insert(
                        id,
                        Inflight { stage: Arc::clone(&stage), payload: payload.clone(), taken_at_ms: now_ms(), key: Some(Arc::clone(&key)) },
                    );
                    drop(g); // release before send — no I/O or wakeup under lock
                    match tx.send((id, payload)) {
                        Ok(()) => {
                            // One take per hand-off, counted where the item enters in flight (R11-10).
                            state.take_total.fetch_add(1, Ordering::Relaxed);
                            if let Some(wal) = &self.wal {
                                wal.append(&Record::Take { id, taken_at_ms: now_ms() })?;
                            }
                            return Ok(true);
                        }
                        Err(p) => {
                            // The keyed waiter timed out or was dropped — undo, try the next or store.
                            self.inflight.lock().remove(&id);
                            payload = p.1;
                            undone = true;
                            g = state.inner.lock();
                        }
                    }
                }
            }
        }
    }

    /// Removes the closed senders parked on `key` (timed-out or dropped keyed takes), keeping
    /// `waiters_count` exact for them, and the map entry when its queue is empty — so a cold key
    /// (a per-job correlation nobody touches again) leaves nothing behind. Under the stage lock.
    fn prune_keyed_waiters(state: &StageState, g: &mut StageInner, key: &str) {
        if let Some(q) = g.keyed_waiters.get_mut(key) {
            let before = q.len();
            q.retain(|tx| !tx.is_closed());
            let pruned = before - q.len();
            if q.is_empty() {
                g.keyed_waiters.remove(key);
            }
            if pruned > 0 {
                state.waiters_count.fetch_sub(pruned as u32, Ordering::Relaxed);
            }
        }
    }

    /// Blocking keyed claim (WS-G / M13). Claims the item on `stage` whose correlation key is `key`,
    /// or parks a keyed waiter until one arrives or `timeout` elapses. O(1) exact-match — never
    /// template matching (that is the blackboard companion's territory). Claims the oldest item
    /// under `key`; a parked waiter joins the back of `key`'s waiter queue (ER7).
    pub(crate) async fn take_by_key(
        &self,
        stage: &str,
        key: &str,
        timeout: Duration,
    ) -> Result<(u64, Bytes), TupleError> {
        let state = self.stage(stage);
        let key: Arc<str> = Arc::from(key);
        let rx = {
            let mut g = state.inner.lock();
            let popped = match g.keyed_entries.get_mut(&key) {
                Some(q) => {
                    let item = q.pop_front();
                    if q.is_empty() {
                        g.keyed_entries.remove(&key);
                    }
                    item
                }
                None => None,
            };
            if let Some((id, payload, enqueued)) = popped {
                state.depth.fetch_sub(1, Ordering::Relaxed);
                state.record_queue_wait(enqueued.elapsed());
                self.inflight.lock().insert(
                    id,
                    Inflight { stage: Arc::from(stage), payload: payload.clone(), taken_at_ms: now_ms(), key: Some(Arc::clone(&key)) },
                );
                drop(g);
                if let Some(wal) = &self.wal {
                    wal.append(&Record::Take { id, taken_at_ms: now_ms() })?;
                }
                // Counted once the take is logged: a refused append returns `Err`, not the item.
                state.take_total.fetch_add(1, Ordering::Relaxed);
                return Ok((id, payload));
            }
            // Join the back of the key's waiter queue, pruning dead senders first.
            Self::prune_keyed_waiters(&state, &mut g, &key);
            let (tx, rx) = oneshot::channel();
            g.keyed_waiters.entry(Arc::clone(&key)).or_default().push_back(tx);
            state.waiters_count.fetch_add(1, Ordering::Relaxed);
            rx
        };
        // From here, however this future ends — served, timed out, or dropped (a cancelled task, a
        // gateway handler whose client went away) — the guard's drop removes our registration unless
        // a dispatcher already took it.
        let mut wait = KeyedWait { state: Arc::clone(&state), key, rx, armed: true };
        tokio::select! {
            // The dispatcher counted the hand-off in `take_total` when it sent, and took our sender.
            item = &mut wait.rx => {
                if item.is_ok() {
                    wait.armed = false;
                }
                item.map_err(|_| TupleError::Timeout) // sender dropped: store shutdown
            }
            _ = tokio::time::sleep(timeout) => {
                // Close, then drain (as in `take`): a dispatcher holding our sender now fails its send
                // and keeps the item; one that sent before the close handed it to us. On `Err` the
                // guard prunes our closed sender — not whichever waiter is present.
                wait.rx.close();
                let item = wait.rx.try_recv();
                if item.is_ok() {
                    wait.armed = false;
                }
                item.map_err(|_| TupleError::Timeout)
            }
        }
    }

    /// Terminal ack: removes the in-flight entry and writes `AckRecord`.
    pub(crate) fn ack(&self, id: u64) -> Result<(), TupleError> {
        let removed = self.inflight.lock().remove(&id);
        match removed {
            None => Err(TupleError::NotFound),
            Some(_) => {
                #[cfg(test)]
                self.between_append_and_apply();
                if let Some(wal) = &self.wal {
                    wal.append(&Record::Ack { id })?;
                    wal.note_acked();
                }
                Ok(())
            }
        }
    }

    /// Atomic pipeline advance: acks `id` AND enqueues `payload` on
    /// `next_stage` under a single `CompleteRecord` — replay can never apply
    /// half of the transition.
    pub(crate) fn complete(
        &self,
        id: u64,
        next_stage: &str,
        payload: Bytes,
    ) -> Result<u64, TupleError> {
        // Claim the old item first; if it already expired back to the queue
        // (worker overran worker_timeout_secs) the complete is refused and
        // the at-least-once re-queue path owns the item.
        let removed = self.inflight.lock().remove(&id);
        if removed.is_none() {
            return Err(TupleError::NotFound);
        }
        let new_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Some(wal) = &self.wal {
            wal.append(&Record::Complete {
                old_id: id,
                new_id,
                stage: Arc::from(next_stage),
                payload: payload.clone(),
                key: None,
            })?;
            wal.note_acked();
        }
        let state = self.stage(next_stage);
        state.put_total.fetch_add(1, Ordering::Relaxed);
        let hot = self.dispatch(&state, Arc::from(next_stage), new_id, payload)?;
        if hot {
            // Release: the dispatch's `take_total` increment for this hand-off precedes this one in
            // program order, so a snapshot that Acquire-loads `hot_total` first sees it.
            state.hot_total.fetch_add(1, Ordering::Release);
        }
        Ok(new_id)
    }

    /// Atomic keyed pipeline advance (WS-G / M13): acks `id` AND puts `payload` on `next_stage`
    /// under correlation `key` in one `CompleteRecord` — the keyed analogue of [`complete`](Self::complete).
    pub(crate) fn complete_keyed(
        &self,
        id: u64,
        next_stage: &str,
        key: Arc<str>,
        payload: Bytes,
    ) -> Result<u64, TupleError> {
        let removed = self.inflight.lock().remove(&id);
        if removed.is_none() {
            return Err(TupleError::NotFound);
        }
        let new_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Some(wal) = &self.wal {
            wal.append(&Record::Complete {
                old_id: id,
                new_id,
                stage: Arc::from(next_stage),
                payload: payload.clone(),
                key: Some(Arc::clone(&key)),
            })?;
            wal.note_acked();
        }
        let state = self.stage(next_stage);
        state.put_total.fetch_add(1, Ordering::Relaxed);
        let hot = self.dispatch_keyed(&state, Arc::from(next_stage), key, new_id, payload)?;
        if hot {
            // Release: the dispatch's `take_total` increment for this hand-off precedes this one in
            // program order, so a snapshot that Acquire-loads `hot_total` first sees it.
            state.hot_total.fetch_add(1, Ordering::Release);
        }
        Ok(new_id)
    }

    /// Fence `next_id` at `n` or above (ER6-T, the fence half): a mirror fences past **every** id a
    /// replicated or replayed record names — an `Ack`, a `Take`, a `Complete`'s old id, the lone
    /// record a compaction keeps for the high-water mark, the lone `Take` a state chunk ends with —
    /// not only a `Put`'s, so a promoted mirror never reissues an id the primary acked. When the
    /// fence moves on a WAL-backed store it is persisted as a lone `Take { n - 1 }`: set, then logged;
    /// replay folds it into the high-water mark and a `Take` never terminates an item. `n` at or
    /// below the current next id changes nothing and logs nothing.
    pub(crate) fn fence_past(&self, n: u64) -> Result<bool, TupleError> {
        let prev = self.next_id.fetch_max(n, Ordering::Relaxed);
        if prev >= n {
            return Ok(false);
        }
        if let Some(wal) = &self.wal {
            wal.append(&Record::Take { id: n - 1, taken_at_ms: 0 })?;
        }
        Ok(true)
    }

    /// Applies a replicated or replayed item under its ORIGINAL id: enqueue
    /// bypassing the watermark (replication must not drop), and fence
    /// `next_id` past it so a promoted secondary never re-issues an id that
    /// the old primary already assigned.
    pub(crate) fn put_with_id(
        &self,
        stage: &str,
        id: u64,
        payload: Bytes,
        key: Option<Arc<str>>,
    ) -> Result<(), TupleError> {
        self.next_id.fetch_max(id + 1, Ordering::Relaxed);
        if let Some(wal) = &self.wal {
            wal.append(&Record::Put {
                id,
                stage: Arc::from(stage),
                payload: payload.clone(),
                key: key.clone(),
            })?;
        }
        let state = self.stage(stage);
        state.put_total.fetch_add(1, Ordering::Relaxed);
        // A keyed replicated item routes into the keyed index so a take_by_key on a promoted mirror
        // still rendezvouses (G1b).
        let hot = match key {
            Some(k) => self.dispatch_keyed(&state, Arc::from(stage), k, id, payload)?,
            None => self.dispatch(&state, Arc::from(stage), id, payload)?,
        };
        if hot {
            // Release: the dispatch's `take_total` increment for this hand-off precedes this one in
            // program order, so a snapshot that Acquire-loads `hot_total` first sees it.
            state.hot_total.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }

    /// Removes a queued (not in-flight) item by id — used when a mirror
    /// applies a replicated ack/complete for an item it holds queued.
    /// Returns `true` when the item was found and removed.
    pub(crate) fn remove_queued(&self, stage: &str, id: u64) -> bool {
        let state = self.stage(stage);
        let mut g = state.inner.lock();
        let before = g.entries.len();
        g.entries.retain(|(eid, _, _)| *eid != id);
        let mut removed = before - g.entries.len();
        // A keyed queued item (M13) lives in the keyed index, not the FIFO.
        if removed == 0 {
            let key = g
                .keyed_entries
                .iter()
                .find(|(_, q)| q.iter().any(|(eid, _, _)| *eid == id))
                .map(|(k, _)| Arc::clone(k));
            if let Some(k) = key {
                if let Some(q) = g.keyed_entries.get_mut(&k) {
                    q.retain(|(eid, _, _)| *eid != id);
                    if q.is_empty() {
                        g.keyed_entries.remove(&k);
                    }
                }
                removed = 1;
            }
        }
        if removed > 0 {
            state.depth.fetch_sub(removed as u32, Ordering::Relaxed);
            if let Some(wal) = &self.wal {
                drop(g);
                // Count the terminal record only when the log has it. A refused one (a poisoned
                // writer) leaves the item live in the log: a restart re-queues it — at-least-once,
                // never a loss.
                match wal.append(&Record::Ack { id }) {
                    Ok(()) => wal.note_acked(),
                    Err(e) => tracing::error!(id, error = %e, "tuple-space: mirror ack not logged"),
                }
            }
            true
        } else {
            false
        }
    }

    /// Re-queues every in-flight item older than `timeout`. Returns the ids
    /// re-queued. No WAL record is written: the item's `PutRecord` is still
    /// live (no ack), so replay already re-queues it.
    pub(crate) fn requeue_expired(&self, timeout: Duration) -> Vec<u64> {
        let cutoff = now_ms().saturating_sub(timeout.as_millis() as u64);
        let expired: Vec<(u64, Inflight)> = {
            let mut g = self.inflight.lock();
            let ids: Vec<u64> = g
                .iter()
                .filter(|(_, v)| v.taken_at_ms <= cutoff)
                .map(|(k, _)| *k)
                .collect();
            ids.into_iter()
                .filter_map(|id| g.remove(&id).map(|v| (id, v)))
                .collect()
        }; // inflight lock released before dispatch re-acquires stage → inflight
        let mut requeued = Vec::with_capacity(expired.len());
        for (id, item) in expired {
            let state = self.stage(&item.stage);
            // A keyed item re-queues under its key (M13) so its waiter still rendezvouses — at the
            // back of the key's queue, as a plain requeue goes to the back of the lane (ER7).
            let ok = match item.key {
                Some(k) => self.dispatch_keyed(&state, Arc::clone(&item.stage), k, id, item.payload).is_ok(),
                None => self.dispatch(&state, Arc::clone(&item.stage), id, item.payload).is_ok(),
            };
            if ok {
                requeued.push(id);
            }
        }
        requeued
    }

    /// Lock-free depth snapshot for one stage or all stages.
    pub(crate) fn depth(&self, stage: Option<&str>) -> Vec<StageDepth> {
        let map = self.stages.pin();
        map.iter()
            .filter(|(name, _)| stage.is_none_or(|s| s == name.as_ref()))
            .map(|(name, state)| StageDepth {
                stage: Arc::clone(name),
                depth: state.depth.load(Ordering::Relaxed),
                waiters: state.waiters_count.load(Ordering::Relaxed),
            })
            .collect()
    }

    /// Full per-stage counter snapshot for the metrics writer.
    pub(crate) fn metrics_snapshot(&self) -> Vec<StageMetrics> {
        let by_stage = self.inflight_by_stage();
        self.stage_states()
            .into_iter()
            .map(|(stage, state)| StageMetrics {
                inflight: by_stage.get(stage.as_ref()).copied().unwrap_or(0),
                depth: state.depth.load(Ordering::Relaxed),
                waiters: state.waiters_count.load(Ordering::Relaxed),
                put_total: state.put_total.load(Ordering::Relaxed),
                // `hot_total` first, with Acquire: every `hot_total` increment (put, put_keyed,
                // complete, complete_keyed, put_with_id — checked at each site) is a Release that
                // follows, in program order, the dispatch's `take_total` increment for the same
                // hand-off, so the later `take_total` load sees at least as many: `hot_total <=
                // take_total` in every snapshot, on weak memory models too.
                hot_total: state.hot_total.load(Ordering::Acquire),
                take_total: state.take_total.load(Ordering::Relaxed),
                rejected_total: state.rejected_total.load(Ordering::Relaxed),
                queue_p99_us: state.queue_p99_us(),
                stage,
            })
            .collect()
    }

    /// Admission at each stage — what was admitted, what was refused at the watermark, what was
    /// taken — for one stage or all (item 4 PR 5). Lock-free, like `depth`.
    pub(crate) fn admission(&self, stage: Option<&str>) -> Vec<crate::StageAdmission> {
        let map = self.stages.pin();
        map.iter()
            .filter(|(name, _)| stage.is_none_or(|s| s == name.as_ref()))
            .map(|(name, state)| crate::StageAdmission {
                stage: Arc::clone(name),
                admitted: state.put_total.load(Ordering::Relaxed),
                rejected: state.rejected_total.load(Ordering::Relaxed),
                taken: state.take_total.load(Ordering::Relaxed),
                high_watermark: self.high_watermark,
            })
            .collect()
    }

    /// Exercised by unit tests; the per-stage view is in `metrics_snapshot`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn inflight_count(&self) -> usize {
        self.inflight.lock().len()
    }

    /// In-flight counts grouped by stage, for depth reporting.
    pub(crate) fn inflight_by_stage(&self) -> HashMap<Arc<str>, u32> {
        let g = self.inflight.lock();
        let mut out: HashMap<Arc<str>, u32> = HashMap::new();
        for item in g.values() {
            *out.entry(Arc::clone(&item.stage)).or_insert(0) += 1;
        }
        out
    }

    pub(crate) fn stage_states(&self) -> Vec<(Arc<str>, Arc<StageState>)> {
        let map = self.stages.pin();
        map.iter()
            .map(|(k, v)| (Arc::clone(k), Arc::clone(v)))
            .collect()
    }

    pub(crate) fn wal_bytes(&self) -> u64 {
        self.wal.as_ref().map_or(0, WalWriter::file_len)
    }

    /// True when more than half the logged items are acked — caller should
    /// invoke [`compact_now`](Self::compact_now).
    pub(crate) fn wants_compaction(&self) -> bool {
        self.wal.as_ref().is_some_and(WalWriter::wants_compaction)
    }

    /// Rewrites the WAL to contain only live items (queued + in-flight), folded from
    /// the log itself, and atomically swaps it in. Holds the WAL lock for the duration
    /// and no other; appends block but the in-memory hot path does not.
    pub(crate) fn compact_now(&self) -> io::Result<()> {
        let Some(wal) = &self.wal else { return Ok(()) };
        wal.compact()
    }

    /// `(epoch, head)` of the WAL, or `(0, 0)` for a transient store.
    pub(crate) fn wal_position(&self) -> (u64, u64) {
        self.wal.as_ref().map_or((0, 0), WalWriter::position)
    }

    /// Current-state chunk for the join-time backfill of a **transient** (WAL-less) store:
    /// live items (queued FIFO + keyed + inflight) with `id >= from_id`, encoded as `Put`
    /// records, ordered by id, bounded by `max_entries`/`max_bytes`. The id is the pagination
    /// cursor (ids are monotone from `next_id`), so items put mid-scan are still picked up and
    /// items acked mid-scan simply drop out — at-least-once, deduped by the mirror's apply.
    /// Returns `(raw, next_id, done)`. Lock discipline: one stage lock at a time, then the
    /// inflight lock — sequential, never nested.
    pub(crate) fn state_chunk(
        &self,
        from_id: u64,
        max_entries: usize,
        max_bytes: usize,
    ) -> (Vec<u8>, u64, bool) {
        // (id, stage, payload, correlation key) — one live item's snapshot.
        type LiveItem = (u64, Arc<str>, Bytes, Option<Arc<str>>);
        let mut items: Vec<LiveItem> = Vec::new();
        {
            let guard = self.stages.pin();
            for (stage, st) in guard.iter() {
                let g = st.inner.lock();
                for (id, payload, _) in g.entries.iter() {
                    if *id >= from_id {
                        items.push((*id, Arc::clone(stage), payload.clone(), None));
                    }
                }
                for (key, q) in g.keyed_entries.iter() {
                    for (id, payload, _) in q.iter() {
                        if *id >= from_id {
                            items.push((*id, Arc::clone(stage), payload.clone(), Some(Arc::clone(key))));
                        }
                    }
                }
            }
        }
        {
            let g = self.inflight.lock();
            for (id, inf) in g.iter() {
                if *id >= from_id {
                    items.push((*id, Arc::clone(&inf.stage), inf.payload.clone(), inf.key.clone()));
                }
            }
        }
        items.sort_unstable_by_key(|(id, ..)| *id);
        // Read after the scan, so it is past every id the chunk names (ER6-T).
        let issued_next = self.next_id.load(Ordering::Relaxed);
        let mut raw = Vec::new();
        let mut next = from_id;
        for (n, (id, stage, payload, key)) in items.into_iter().enumerate() {
            if n >= max_entries || raw.len() >= max_bytes {
                return (raw, next, false);
            }
            Record::Put { id, stage, payload, key }.encode(&mut raw);
            next = id + 1;
        }
        // The last chunk ends with a lone `Take` naming the last id this store issued (ER6-T): live
        // `Put`s alone fence a mirror only past the highest *live* id. A `Take`, not the `Ack` the
        // design named: an `Ack` for that id would remove it from a mirror when it is live (in this
        // chunk) and make the mirror refuse its replicated `Put` when it is put after the scan; a
        // `Take` changes no item's liveness, and a mirror older than ER6-T ignores it.
        if let Some(last) = issued_next.checked_sub(1) {
            Record::Take { id: last, taken_at_ms: 0 }.encode(&mut raw);
        }
        (raw, next, true)
    }

    /// Serves a replay chunk from the WAL. `None` for a transient store.
    pub(crate) fn wal_read_chunk(
        &self,
        expect_epoch: u64,
        from_offset: u64,
        max_entries: usize,
        max_bytes: usize,
    ) -> io::Result<Option<WalChunkData>> {
        match &self.wal {
            None => Ok(None),
            Some(wal) => wal
                .read_chunk(expect_epoch, from_offset, max_entries, max_bytes)
                .map(Some),
        }
    }

    /// One maintenance pass: the periodic sync, then a compaction when one is wanted.
    /// Each step runs whatever the other did: a failing sync must not keep a poisoned writer from its
    /// repair. The first error is returned.
    pub(crate) fn maintenance_tick(&self, force: bool) -> io::Result<()> {
        let synced = self.checkpoint_if_due(force);
        let compacted = if self.wants_compaction() { self.compact_now() } else { Ok(()) };
        synced.and(compacted)
    }

    /// Test seams over the WAL: inject the next append's failure; make every compaction / sync fail
    /// while set; read the poison and the terminal-record count.
    #[cfg(test)]
    pub(crate) fn inject_append_fault(&self) {
        self.wal.as_ref().unwrap().fault.store(true, Ordering::SeqCst);
    }
    #[cfg(test)]
    pub(crate) fn set_compact_fault(&self, on: bool) {
        self.wal.as_ref().unwrap().compact_fault.store(on, Ordering::SeqCst);
    }
    #[cfg(test)]
    pub(crate) fn set_sync_fault(&self, on: bool) {
        self.wal.as_ref().unwrap().sync_fault.store(on, Ordering::SeqCst);
    }
    /// True while the WAL refuses appends (poisoned by a failed write, until a compaction repairs
    /// it). `false` for a transient store.
    pub(crate) fn wal_refusing(&self) -> bool {
        self.wal.as_ref().is_some_and(|w| w.inner.lock().poison.is_some())
    }
    #[cfg(test)]
    pub(crate) fn wal_poisoned(&self) -> bool {
        self.wal_refusing()
    }
    #[cfg(test)]
    pub(crate) fn set_post_rename_fault(&self, on: bool) {
        self.wal.as_ref().unwrap().post_rename_fault.store(on, Ordering::SeqCst);
    }
    #[cfg(test)]
    pub(crate) fn wal_acked_count(&self) -> u64 {
        self.wal.as_ref().unwrap().inner.lock().acked
    }

    /// Fsync the WAL when the ops threshold is reached, or unconditionally
    /// when `force` is set (periodic 1 s safety sync, shutdown). Cheap no-op
    /// when nothing is pending.
    pub(crate) fn checkpoint_if_due(&self, force: bool) -> io::Result<()> {
        match &self.wal {
            Some(wal) => wal.sync_if_due(force),
            None => Ok(()),
        }
    }
}

/// The last base [`fresh_id_base`] handed out in this process, so two stores created in the same
/// millisecond still get disjoint bases.
static LAST_FRESH_BASE: AtomicU64 = AtomicU64::new(0);

/// The first id of an id space with **no history** — a transient store, or a WAL that names no id
/// (ER3, R4-01). An item's id is its identity everywhere: the effects companion's `operation_id`
/// (`tuple/{ns}/{stage}/{id}`), the advisory in-flight key, a mirror's `admit`. A space that started
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
/// **Keep in sync with `fresh_id_base` in `mycelium-blackboard/src/store.rs`** — the same rule, copied, each
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

/// Unix-ms now, through the substrate's replay seam (`mycelium::sim_seam::wall_now_ms`, the one
/// wall clock every `now_ms` reads — post-360 row J, 2026-10-12; a direct `SystemTime::now()`
/// before it). Saturates to 0 before the epoch.
pub(crate) fn now_ms() -> u64 {
    mycelium::sim_seam::wall_now_ms()
}

// ─── WAL ─────────────────────────────────────────────────────────────────────

/// WAL file header: magic + format version. A file whose version is newer
/// than this build understands is REFUSED at open — never truncated. Without
/// the header, a future format change would read as a torn tail and be
/// silently truncated: an upgrade data-loss hazard (Run-17 finding,
/// Evolvability).
const WAL_MAGIC: &[u8; 6] = b"MTSWAL";
/// v2 (WS-G / M13) adds the keyed record kinds (`REC_PUT_KEYED` / `REC_COMPLETE_KEYED`). Unkeyed
/// records are byte-identical to v1, so a v1 WAL replays unchanged and a v2 WAL with no keyed items
/// is byte-identical to a v1 one. v1 is accepted on read (rolling-upgrade window).
const WAL_VERSION: u16 = 2;
const PREV_WAL_VERSION: u16 = 1;
const WAL_HEADER_LEN: u64 = 8; // magic + u16 LE version

/// The back-off ceiling for a failing compaction, in maintenance ticks: 50 × 200 ms ≈ 10 s.
const MAX_COMPACTION_SKIP: u32 = 50;

fn wal_header() -> [u8; WAL_HEADER_LEN as usize] {
    let mut h = [0u8; WAL_HEADER_LEN as usize];
    h[..6].copy_from_slice(WAL_MAGIC);
    h[6..8].copy_from_slice(&WAL_VERSION.to_le_bytes());
    h
}

const REC_PUT: u8 = 1;
const REC_TAKE: u8 = 2;
const REC_ACK: u8 = 3;
const REC_COMPLETE: u8 = 4;
// WS-G / M13 (WAL v2): keyed variants carry a correlation key after the stage.
const REC_PUT_KEYED: u8 = 5;
const REC_COMPLETE_KEYED: u8 = 6;

/// One chunk of raw WAL records served to a replaying secondary.
pub(crate) struct WalChunkData {
    pub epoch: u64,
    pub raw: Vec<u8>,
    pub next_offset: u64,
    pub done: bool,
}

/// One WAL record. `Complete` is a distinct type (not Ack + Put) so replay
/// applies the stage transition atomically — see the plan doc for the
/// duplicate hazard the split encoding reintroduces.
#[derive(Debug)]
pub(crate) enum Record {
    Put { id: u64, stage: Arc<str>, payload: Bytes, key: Option<Arc<str>> },
    Take { id: u64, taken_at_ms: u64 },
    Ack { id: u64 },
    Complete { old_id: u64, new_id: u64, stage: Arc<str>, payload: Bytes, key: Option<Arc<str>> },
}

impl Record {
    pub(crate) fn encode(&self, buf: &mut Vec<u8>) {
        let body_start = buf.len() + 5; // [kind u8][len u32]
        match self {
            Record::Put { id, stage, payload, key } => {
                buf.push(if key.is_some() { REC_PUT_KEYED } else { REC_PUT });
                buf.extend_from_slice(&[0; 4]);
                buf.extend_from_slice(&id.to_le_bytes());
                buf.extend_from_slice(&(stage.len() as u16).to_le_bytes());
                buf.extend_from_slice(stage.as_bytes());
                if let Some(k) = key {
                    buf.extend_from_slice(&(k.len() as u16).to_le_bytes());
                    buf.extend_from_slice(k.as_bytes());
                }
                buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                buf.extend_from_slice(payload);
            }
            Record::Take { id, taken_at_ms } => {
                buf.push(REC_TAKE);
                buf.extend_from_slice(&[0; 4]);
                buf.extend_from_slice(&id.to_le_bytes());
                buf.extend_from_slice(&taken_at_ms.to_le_bytes());
            }
            Record::Ack { id } => {
                buf.push(REC_ACK);
                buf.extend_from_slice(&[0; 4]);
                buf.extend_from_slice(&id.to_le_bytes());
            }
            Record::Complete { old_id, new_id, stage, payload, key } => {
                buf.push(if key.is_some() { REC_COMPLETE_KEYED } else { REC_COMPLETE });
                buf.extend_from_slice(&[0; 4]);
                buf.extend_from_slice(&old_id.to_le_bytes());
                buf.extend_from_slice(&new_id.to_le_bytes());
                buf.extend_from_slice(&(stage.len() as u16).to_le_bytes());
                buf.extend_from_slice(stage.as_bytes());
                if let Some(k) = key {
                    buf.extend_from_slice(&(k.len() as u16).to_le_bytes());
                    buf.extend_from_slice(k.as_bytes());
                }
                buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                buf.extend_from_slice(payload);
            }
        }
        let body_len = (buf.len() - body_start) as u32;
        buf[body_start - 4..body_start].copy_from_slice(&body_len.to_le_bytes());
    }

    /// Decodes one record from `data`. Returns `(record, bytes_consumed)`,
    /// or `None` when the remaining bytes are a truncated tail.
    fn decode(data: &[u8]) -> Option<(Record, usize)> {
        if data.len() < 5 {
            return None;
        }
        let kind = data[0];
        let body_len = u32::from_le_bytes(data[1..5].try_into().ok()?) as usize;
        let body = data.get(5..5 + body_len)?;
        let rec = match kind {
            REC_PUT | REC_PUT_KEYED => {
                let id = u64::from_le_bytes(body.get(..8)?.try_into().ok()?);
                let stage_len =
                    u16::from_le_bytes(body.get(8..10)?.try_into().ok()?) as usize;
                let stage = std::str::from_utf8(body.get(10..10 + stage_len)?).ok()?;
                let mut p = 10 + stage_len;
                let key = if kind == REC_PUT_KEYED {
                    let kl = u16::from_le_bytes(body.get(p..p + 2)?.try_into().ok()?) as usize;
                    let k = std::str::from_utf8(body.get(p + 2..p + 2 + kl)?).ok()?;
                    p += 2 + kl;
                    Some(Arc::from(k))
                } else {
                    None
                };
                let payload_len =
                    u32::from_le_bytes(body.get(p..p + 4)?.try_into().ok()?) as usize;
                let payload = body.get(p + 4..p + 4 + payload_len)?;
                Record::Put {
                    id,
                    stage: Arc::from(stage),
                    payload: Bytes::copy_from_slice(payload),
                    key,
                }
            }
            REC_TAKE => Record::Take {
                id: u64::from_le_bytes(body.get(..8)?.try_into().ok()?),
                taken_at_ms: u64::from_le_bytes(body.get(8..16)?.try_into().ok()?),
            },
            REC_ACK => Record::Ack {
                id: u64::from_le_bytes(body.get(..8)?.try_into().ok()?),
            },
            REC_COMPLETE | REC_COMPLETE_KEYED => {
                let old_id = u64::from_le_bytes(body.get(..8)?.try_into().ok()?);
                let new_id = u64::from_le_bytes(body.get(8..16)?.try_into().ok()?);
                let stage_len =
                    u16::from_le_bytes(body.get(16..18)?.try_into().ok()?) as usize;
                let stage = std::str::from_utf8(body.get(18..18 + stage_len)?).ok()?;
                let mut p = 18 + stage_len;
                let key = if kind == REC_COMPLETE_KEYED {
                    let kl = u16::from_le_bytes(body.get(p..p + 2)?.try_into().ok()?) as usize;
                    let k = std::str::from_utf8(body.get(p + 2..p + 2 + kl)?).ok()?;
                    p += 2 + kl;
                    Some(Arc::from(k))
                } else {
                    None
                };
                let payload_len =
                    u32::from_le_bytes(body.get(p..p + 4)?.try_into().ok()?) as usize;
                let payload = body.get(p + 4..p + 4 + payload_len)?;
                Record::Complete {
                    old_id,
                    new_id,
                    stage: Arc::from(stage),
                    payload: Bytes::copy_from_slice(payload),
                    key,
                }
            }
            _ => return None, // unknown kind — treat as corrupt tail
        };
        Some((rec, 5 + body_len))
    }
}

struct WalInner {
    file: File,
    path: PathBuf,
    /// `Some(reason)` after a failed append: part of a frame may be on disk, so the file's end is
    /// unknown and nothing may be appended behind it (row C, mirroring the core WAL's `WriterState`).
    /// Every append is refused by name until a compaction rewrites the file from the live state —
    /// or a reopen truncates the torn tail.
    poison: Option<String>,
    /// Consecutive failed compactions, and the `wants_compaction` calls still to decline before the
    /// next attempt — a compaction that keeps failing (a repair or not) backs off (2, 4 … up to
    /// `MAX_COMPACTION_SKIP` ticks) rather than rereading the whole log on every tick.
    repair_failures: u32,
    repair_skip: u32,
    file_len: u64,
    ops_since_sync: u64,
    /// Live put-side records (Put + the new half of Complete).
    total: u64,
    /// Terminal records (Ack + the old half of Complete).
    acked: u64,
    /// Bumped on every compaction. Byte offsets are only comparable within
    /// one epoch; a replay client whose epoch is stale must restart from 0.
    epoch: u64,
}

pub(crate) struct WalWriter {
    inner: Mutex<WalInner>,
    checkpoint_every: u64,
    file_len_shadow: AtomicU64,
    /// One owner per WAL file: the core's lock on `<wal>.lock`, held for the writer's lifetime.
    _lock: mycelium::OwnershipLock,
    /// Test seam: when set, the next append writes part of its frame and fails — the shape a full
    /// disk or a pulled cable produces, which the production path cannot be made to produce on demand.
    #[cfg(test)]
    fault: std::sync::atomic::AtomicBool,
    /// Test seams: while set, every compaction / every sync fails.
    #[cfg(test)]
    compact_fault: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    sync_fault: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    post_rename_fault: std::sync::atomic::AtomicBool,
}

// Test seam: the storage steps compaction takes, in order, on this thread (compaction is synchronous,
// so the test that runs it reads its own trace). Production records nothing.
#[cfg(test)]
thread_local! {
    static FS_TRACE: std::cell::RefCell<Vec<&'static str>> = const { std::cell::RefCell::new(Vec::new()) };
    /// The directory each open-time sync synced, in order — the witness's exact holders.
    static FS_DIRS: std::cell::RefCell<Vec<(&'static str, PathBuf)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Record an open-time directory sync, from inside it. Production records nothing.
fn fs_dir(_op: &'static str, _dir: &Path) {
    fs_trace(_op);
    #[cfg(test)]
    FS_DIRS.with(|t| t.borrow_mut().push((_op, _dir.to_path_buf())));
}

#[cfg(test)]
thread_local! {
    static COMPACT_BETWEEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn fs_trace(_op: &'static str) {
    #[cfg(test)]
    FS_TRACE.with(|t| t.borrow_mut().push(_op));
}

impl WalWriter {
    /// Opens (or creates) the WAL at `path`, replays it, and returns the
    /// writer plus the live items `(id, stage, payload)` in id order and the
    /// highest id seen (for the `next_id` fence).
    #[allow(clippy::type_complexity)]
    fn open(
        path: &Path,
        checkpoint_every: u64,
    ) -> io::Result<(Self, Vec<(u64, Arc<str>, Bytes, Option<Arc<str>>)>, Option<u64>)> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            // Every directory on the path has its entry synced in the one above it, on every open
            // (external review 2026-10-31 F7, plan row R6; #718's review); the file's own entry is
            // synced once the file is open, below.
            mycelium::create_dir_all_synced_observed(parent, &mut |d| fs_dir("open.mkdir.sync", d))?;
        }
        // Ownership first, repair second, writes last — the core journal's order. A second store on
        // this path, in this process or another, is refused with `WouldBlock` naming the file.
        let lock = mycelium::OwnershipLock::acquire(path)?;
        let mut file = OpenOptions::new()
            .read(true)
            .create(true)
            .append(true)
            .open(path)?;
        let mut data = Vec::new();
        file.seek(SeekFrom::Start(0))?;
        file.read_to_end(&mut data)?;

        // Header handling. Empty file: stamp the current header. Non-empty:
        // the magic must match and the version must be one this build
        // understands — anything else is refused with the file untouched
        // (silent truncation of a future format is the failure mode this
        // header exists to prevent).
        if data.is_empty() {
            file.write_all(&wal_header())?;
            data.extend_from_slice(&wal_header());
            // The header's bytes; the entry naming the file is synced on every open, below.
            file.sync_all()?;
        } else if data.len() < WAL_HEADER_LEN as usize
            || &data[..WAL_MAGIC.len()] != WAL_MAGIC
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} is not a mycelium-tuple-space WAL (missing MTSWAL magic); \
                     refusing to open or modify it",
                    path.display()
                ),
            ));
        } else {
            let version =
                u16::from_le_bytes(data[6..8].try_into().expect("two header bytes"));
            // Accept the current and the previous format (rolling upgrade); refuse a *newer* one
            // (no silent truncation of a format this build does not understand). v1 unkeyed records
            // are byte-identical, so a v1 WAL replays unchanged here.
            if version != WAL_VERSION && version != PREV_WAL_VERSION {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{} is WAL format v{version}, but this build supports v{WAL_VERSION} \
                         (and reads v{PREV_WAL_VERSION}); refusing to open (no silent truncation \
                         of newer formats)",
                        path.display()
                    ),
                ));
            }
        }

        // The entry naming the file, on every open (F7, R6; #718's review, finding 3): a fresh WAL's
        // first synced record must not sit in a file a power loss can leave unnamed, and a reopen
        // covers a file an earlier open created whose sync failed. Told to the test trace from
        // inside the sync.
        mycelium::sync_parent_dir_observed(path, &mut |d| fs_dir("open.parent.sync", d))?;

        let Folded { items, total, acked, end: offset, high_water } = fold_wal(&data).map_err(|offset| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} holds a corrupt record at byte {offset} with data after it; refusing to \
                     open (the records after it cannot be trusted, and truncating would discard \
                     them silently). The file is untouched: move it aside to start empty, or \
                     restore it",
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
                "tuple-space: truncated a torn final record left by a crash mid-append"
            );
        }
        let max_id = high_water;
        let live: Vec<(u64, Arc<str>, Bytes, Option<Arc<str>>)> = items
            .into_iter()
            .filter(|(_, it)| !it.acked)
            .map(|(id, it)| (id, it.stage, it.payload, it.key))
            .collect();
        let file_len = offset as u64;
        let writer = Self {
            inner: Mutex::new(WalInner {
                file,
                path: path.to_path_buf(),
                poison: None,
                repair_failures: 0,
                repair_skip: 0,
                file_len,
                ops_since_sync: 0,
                total,
                acked,
                epoch: 0,
            }),
            checkpoint_every,
            file_len_shadow: AtomicU64::new(file_len),
            _lock: lock,
            #[cfg(test)]
            fault: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            compact_fault: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            sync_fault: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            post_rename_fault: std::sync::atomic::AtomicBool::new(false),
        };
        Ok((writer, live, max_id))
    }

    fn append(&self, rec: &Record) -> Result<(), TupleError> {
        let mut buf = Vec::with_capacity(64);
        rec.encode(&mut buf);
        let mut g = self.inner.lock();
        if let Some(reason) = &g.poison {
            return Err(TupleError::Io(io::Error::other(format!(
                "the tuple-space WAL refuses appends after a failed write ({reason}) until a compaction \
                 rewrites it or a reopen truncates the torn tail"
            ))));
        }
        #[cfg(test)]
        let written = if self.fault.swap(false, Ordering::SeqCst) {
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
                "tuple-space: a WAL append failed; every later append is refused until a compaction \
                 rewrites the log (the next maintenance tick tries one)"
            );
            g.poison = Some(e.to_string());
            return Err(TupleError::Io(e));
        }
        g.file_len += buf.len() as u64;
        g.ops_since_sync += 1;
        if matches!(rec, Record::Put { .. } | Record::Complete { .. }) {
            g.total += 1;
        }
        self.file_len_shadow.store(g.file_len, Ordering::Relaxed);
        Ok(())
    }

    fn note_acked(&self) {
        self.inner.lock().acked += 1;
    }

    fn wants_compaction(&self) -> bool {
        let mut g = self.inner.lock();
        // The last compaction failed and the back-off has not run out.
        if g.repair_skip > 0 {
            g.repair_skip -= 1;
            return false;
        }
        // A poisoned writer is repaired by the rewrite, so it wants one.
        g.poison.is_some() || (g.total > 0 && g.acked * 2 > g.total)
    }

    fn file_len(&self) -> u64 {
        self.file_len_shadow.load(Ordering::Relaxed)
    }

    /// Fsync when the ops threshold is reached, or when `force` is set and
    /// any appends are pending.
    fn sync_if_due(&self, force: bool) -> io::Result<()> {
        #[cfg(test)]
        if self.sync_fault.load(Ordering::SeqCst) {
            return Err(io::Error::other("injected sync failure"));
        }
        let mut g = self.inner.lock();
        let due = g.ops_since_sync >= self.checkpoint_every
            || (force && g.ops_since_sync > 0);
        if due {
            g.file.sync_data()?;
            fs_trace("append.sync");
            g.ops_since_sync = 0;
        }
        Ok(())
    }

    /// Rewrites the log to hold only its live items — **folded from the log itself**, under the WAL
    /// lock, never from memory — and atomically swaps the new file in.
    ///
    /// Row C: a write appends to the WAL and then applies to memory, so a compaction that snapshot
    /// memory could land between the two and rewrite a log without a record whose append had
    /// already been acknowledged. The log holds every acknowledged record by definition, and no
    /// append interleaves while the lock is held, so the rewrite loses none. A record applied to
    /// memory *before* its append (take, ack, requeue) appends to the new file after this returns.
    /// Only the bytes up to `file_len` are folded, so a poisoned writer's torn frame is left behind.
    fn compact(&self) -> io::Result<()> {
        let mut guard = self.inner.lock();
        let result = self.compact_locked(&mut guard);
        match &result {
            Ok(()) => {
                guard.repair_failures = 0;
                guard.repair_skip = 0;
            }
            Err(e) => {
                guard.repair_failures = guard.repair_failures.saturating_add(1);
                guard.repair_skip = (1u32 << guard.repair_failures.min(6)).min(MAX_COMPACTION_SKIP);
                tracing::warn!(
                    error = %e,
                    failures = guard.repair_failures,
                    poisoned = guard.poison.is_some(),
                    "tuple-space: the compaction failed; backing off {} ticks",
                    guard.repair_skip
                );
            }
        }
        result
    }

    fn compact_locked(&self, g: &mut WalInner) -> io::Result<()> {
        #[cfg(test)]
        if self.compact_fault.load(Ordering::SeqCst) {
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
                    g.path.display()
                ),
            )
        })?;
        let mut live: Vec<Record> = Vec::new();
        let mut highest_live = None;
        for (id, it) in folded.items.into_iter().filter(|(_, it)| !it.acked) {
            live.push(Record::Put { id, stage: it.stage, payload: it.payload, key: it.key });
            if let Some(taken_at_ms) = it.taken_at_ms {
                live.push(Record::Take { id, taken_at_ms });
            }
            highest_live = Some(id);
        }
        // The id high-water mark survives the rewrite (#597 review, finding 7): when the highest id
        // the log ever held was acked away, a lone `Ack` for it is kept, so the next open fences
        // `next_id` past it and a restart never reissues an acknowledged id. A lone `Ack` names no
        // item, so replay — this build's or an older one's — changes nothing else for it.
        if let Some(hw) = folded.high_water
            && highest_live.is_none_or(|h| h < hw)
        {
            live.push(Record::Ack { id: hw });
        }
        // Named after the whole WAL file, so two WALs sharing a stem never share a temp file.
        let tmp_path = {
            let mut name = g.path.as_os_str().to_owned();
            name.push(".compact");
            PathBuf::from(name)
        };
        let mut tmp = File::create(&tmp_path)?;
        let mut buf = Vec::new();
        buf.extend_from_slice(&wal_header());
        let mut total = 0u64;
        for rec in &live {
            if matches!(rec, Record::Put { .. } | Record::Complete { .. }) {
                total += 1;
            }
            rec.encode(&mut buf);
        }
        tmp.write_all(&buf)?;
        fs_trace("tmp.write");
        tmp.sync_data()?;
        fs_trace("tmp.sync");
        std::fs::rename(&tmp_path, &g.path)?;
        fs_trace("rename");
        // From here the new log is at `path` and `g.file` is the old, unlinked one: any failure must
        // poison the writer, or appends would go on landing in a file no restart will read
        // (#597 re-review, finding 5). A later compaction folds `g.file` — the old file, which holds
        // every record — and installs again.
        let path = g.path.clone();
        let installed = (|| -> io::Result<File> {
            #[cfg(test)]
            if self.post_rename_fault.load(Ordering::SeqCst) {
                return Err(io::Error::other("injected failure after the rename"));
            }
            // The rename is durable only once the directory is: without this a power loss can bring
            // back the old log — the core snapshot's install order (`sync_data → rename → fsync_dir`).
            fsync_parent(&path)?;
            fs_trace("dir.sync");
            OpenOptions::new().read(true).append(true).open(&path)
        })();
        let file = match installed {
            Ok(file) => file,
            Err(e) => {
                tracing::error!(error = %e, "tuple-space: compaction failed after its rename; appends are refused until a compaction succeeds");
                g.poison = Some(format!("compaction failed after its rename: {e}"));
                return Err(e);
            }
        };
        g.file = file;
        g.file_len = buf.len() as u64;
        g.ops_since_sync = 0;
        g.total = total;
        g.acked = 0;
        g.epoch += 1;
        // The torn frame went with the old file; appends resume behind the rewritten log.
        if let Some(reason) = g.poison.take() {
            tracing::info!(%reason, "tuple-space: a compaction rewrote the WAL after a failed append; appends resume");
        }
        self.file_len_shadow.store(g.file_len, Ordering::Relaxed);
        Ok(())
    }

    /// `(epoch, head)` — the current compaction epoch and byte length.
    fn position(&self) -> (u64, u64) {
        let g = self.inner.lock();
        (g.epoch, g.file_len)
    }

    /// Reads raw record bytes starting at `from_offset`, bounded by
    /// `min(max_entries, max_bytes)` — whichever is hit first. Returns
    /// `(epoch, raw, next_offset, done)`. When `expect_epoch` is stale the
    /// caller's offsets are meaningless: returns the current epoch with no
    /// data and `next_offset = 0` so the client restarts.
    fn read_chunk(
        &self,
        expect_epoch: u64,
        from_offset: u64,
        max_entries: usize,
        max_bytes: usize,
    ) -> io::Result<WalChunkData> {
        let mut g = self.inner.lock();
        if g.epoch != expect_epoch {
            return Ok(WalChunkData {
                epoch: g.epoch,
                raw: Vec::new(),
                next_offset: 0,
                done: false,
            });
        }
        // A cursor of 0 (or anything inside the header) starts at the first
        // record, not at the magic bytes — which would decode as garbage and
        // stall the replay loop.
        let from_offset = from_offset.max(WAL_HEADER_LEN);
        let head = g.file_len;
        if from_offset >= head {
            return Ok(WalChunkData {
                epoch: g.epoch,
                raw: Vec::new(),
                next_offset: head,
                done: true,
            });
        }
        let want = ((head - from_offset) as usize).min(max_bytes.max(64));
        let mut raw = vec![0u8; want];
        // Positioned reads so the append cursor (O_APPEND) is untouched.
        g.file.seek(SeekFrom::Start(from_offset))?;
        let mut filled = 0;
        while filled < want {
            let n = g.file.read(&mut raw[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        raw.truncate(filled);
        // Trim to whole records within the entry budget.
        let mut consumed = 0usize;
        let mut entries = 0usize;
        while consumed < raw.len() && entries < max_entries {
            match Record::decode(&raw[consumed..]) {
                Some((_, n)) => {
                    consumed += n;
                    entries += 1;
                }
                None => break, // partial record at the end of the chunk
            }
        }
        raw.truncate(consumed);
        let next = from_offset + consumed as u64;
        Ok(WalChunkData {
            epoch: g.epoch,
            raw,
            next_offset: next,
            done: next >= head,
        })
    }
}

/// One item's state as the log records it.
struct ItemState {
    stage: Arc<str>,
    payload: Bytes,
    acked: bool,
    key: Option<Arc<str>>,
    /// The latest `Take` for the item, if any (kept by compaction; replay re-queues it regardless).
    taken_at_ms: Option<u64>,
}

/// A WAL image folded to its items: what open replays and compaction rewrites.
struct Folded {
    items: BTreeMap<u64, ItemState>,
    /// Put-side records (Put + the new half of Complete).
    total: u64,
    /// Terminal records (Ack + the old half of Complete).
    acked: u64,
    /// Where the complete records end — short of the image's end only for a torn final frame.
    end: usize,
    /// The highest item id any record names, acked or live — what `next_id` is fenced past.
    high_water: Option<u64>,
}

/// Fold a whole WAL image (header included) record by record. `Err(offset)` names a corrupt record
/// with data after it; a torn final frame ends the fold at `end`.
fn fold_wal(data: &[u8]) -> Result<Folded, usize> {
    let mut items: BTreeMap<u64, ItemState> = BTreeMap::new();
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
                let named = match &rec {
                    Record::Put { id, .. } | Record::Take { id, .. } | Record::Ack { id } => *id,
                    Record::Complete { old_id, new_id, .. } => (*old_id).max(*new_id),
                };
                high_water = high_water.max(Some(named));
                match rec {
                    Record::Put { id, stage, payload, key } => {
                        total += 1;
                        items.insert(id, ItemState { stage, payload, acked: false, key, taken_at_ms: None });
                    }
                    // Take alone never terminates an item: taken-but-unacked re-queues as abandoned.
                    Record::Take { id, taken_at_ms } => {
                        if let Some(it) = items.get_mut(&id) {
                            it.taken_at_ms = Some(taken_at_ms);
                        }
                    }
                    // Counted only when it terminates an item the log holds: the lone `Ack` a
                    // compaction keeps for the high-water mark is not a terminal record.
                    Record::Ack { id } => {
                        if let Some(it) = items.get_mut(&id) {
                            acked += 1;
                            it.acked = true;
                        }
                    }
                    Record::Complete { old_id, new_id, stage, payload, key } => {
                        acked += 1;
                        total += 1;
                        if let Some(it) = items.get_mut(&old_id) {
                            it.acked = true;
                        }
                        items.insert(new_id, ItemState { stage, payload, acked: false, key, taken_at_ms: None });
                    }
                }
            }
        }
    }
    Ok(Folded { items, total, acked, end: offset, high_water })
}

/// What a WAL image holds at a record boundary — the difference between a crash and corruption
/// (row C, after the core's `WalEnd`).
enum Frame {
    /// A whole record, and the bytes it took.
    Record(Record, usize),
    /// The image ends inside this record: a crash mid-append. Everything before it is good.
    Torn,
    /// A record that is all there and does not decode, or bytes that are not a record at all, with
    /// data after them. Nothing after it can be trusted — and nothing after it may be truncated away.
    Corrupt,
}

fn scan_frame(data: &[u8]) -> Frame {
    if let Some((rec, n)) = Record::decode(data) {
        return Frame::Record(rec, n);
    }
    if !(REC_PUT..=REC_COMPLETE_KEYED).contains(&data[0]) {
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
    // phantom record (an `Ack` of id 0, a `Put` with a zeroed payload) replayed as if written. That
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

/// Decodes a raw record stream (as served by `wal_read_chunk`) back into
/// records. Stops at the first malformed/partial record.
pub(crate) fn decode_records(data: &[u8]) -> Vec<Record> {
    let mut out = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        match Record::decode(&data[offset..]) {
            Some((rec, n)) => {
                out.push(rec);
                offset += n;
            }
            None => break,
        }
    }
    out
}

/// Test-only: read every record currently in the WAL file at `path`,
/// skipping the file header.
#[cfg(test)]
pub(crate) fn read_wal(path: &Path) -> Vec<Record> {
    let data = std::fs::read(path).unwrap_or_default();
    let mut out = Vec::new();
    let mut offset = WAL_HEADER_LEN as usize;
    while offset < data.len() {
        match Record::decode(&data[offset..]) {
            Some((rec, consumed)) => {
                out.push(rec);
                offset += consumed;
            }
            None => break,
        }
    }
    out
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_wal(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "mts-wal-{}-{}.log",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn b(s: &str) -> Bytes {
        Bytes::copy_from_slice(s.as_bytes())
    }

    #[tokio::test]
    async fn put_then_take() {
        let store = TupleStore::transient(500);
        let id_a = store.put("s", b("alpha")).unwrap();
        let id_b = store.put("s", b("beta")).unwrap();
        let (got_a, pa) = store.take("s", Duration::from_millis(100)).await.unwrap();
        let (got_b, pb) = store.take("s", Duration::from_millis(100)).await.unwrap();
        // FIFO order guaranteed by the VecDeque.
        assert_eq!((got_a, pa.as_ref()), (id_a, &b"alpha"[..]));
        assert_eq!((got_b, pb.as_ref()), (id_b, &b"beta"[..]));
        assert_eq!(store.depth(Some("s"))[0].depth, 0);
    }

    // ── M13 / WS-G · G-G1a: keyed-exact-match rendezvous ────────────────────

    #[tokio::test]
    async fn keyed_put_then_take_by_key() {
        let store = TupleStore::transient(500);
        let id = store.put_keyed("po", Arc::from("inv-42"), b("purchase-order")).unwrap();
        // The wrong key does not match (times out fast); the right key claims it.
        assert!(store.take_by_key("po", "inv-99", Duration::from_millis(50)).await.is_err());
        let (got, payload) = store.take_by_key("po", "inv-42", Duration::from_millis(100)).await.unwrap();
        assert_eq!((got, payload.as_ref()), (id, &b"purchase-order"[..]));
        assert_eq!(store.depth(Some("po"))[0].depth, 0, "keyed take decrements depth");
    }

    #[tokio::test]
    async fn keyed_take_parks_until_keyed_put() {
        // The two-stream join: a worker waits on a correlation key before the item arrives.
        let store = Arc::new(TupleStore::transient(500));
        let s2 = Arc::clone(&store);
        let waiter = tokio::spawn(async move {
            s2.take_by_key("po", "inv-7", Duration::from_secs(5)).await
        });
        tokio::time::sleep(Duration::from_millis(50)).await; // ensure the waiter parks first
        let id = store.put_keyed("po", Arc::from("inv-7"), b("matched")).unwrap();
        let (got, payload) = waiter.await.unwrap().unwrap();
        assert_eq!((got, payload.as_ref()), (id, &b"matched"[..]));
    }

    #[tokio::test]
    async fn keyed_and_unkeyed_lanes_do_not_interfere() {
        let store = TupleStore::transient(500);
        // A keyed item is NOT claimable by an unkeyed FIFO take, and vice versa.
        store.put_keyed("s", Arc::from("k1"), b("keyed")).unwrap();
        assert!(store.take("s", Duration::from_millis(50)).await.is_err(),
            "an unkeyed take must not claim a keyed item");
        let id_u = store.put("s", b("unkeyed")).unwrap();
        assert!(store.take_by_key("s", "k1", Duration::from_millis(50)).await.is_ok(),
            "the keyed item is still claimable by its key");
        let (got, _) = store.take("s", Duration::from_millis(50)).await.unwrap();
        assert_eq!(got, id_u, "the unkeyed item is claimable by an unkeyed take");
    }

    #[tokio::test]
    async fn keyed_complete_advances_under_key() {
        let store = TupleStore::transient(500);
        let id = store.put_keyed("a", Arc::from("corr"), b("v1")).unwrap();
        let (claimed, _) = store.take_by_key("a", "corr", Duration::from_millis(100)).await.unwrap();
        assert_eq!(claimed, id);
        // complete_keyed advances the item onto stage "b" under the same correlation key.
        store.complete_keyed(id, "b", Arc::from("corr"), b("v2")).unwrap();
        let (_, payload) = store.take_by_key("b", "corr", Duration::from_millis(100)).await.unwrap();
        assert_eq!(payload.as_ref(), &b"v2"[..]);
    }

    // ── ER7 (R11-03, R11-10, R11-11): a queue per key, for items and for waiters ──

    /// Lets every spawned task on this current-thread runtime run to its first park.
    async fn let_park() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    /// Takes every item queued under `key`, in order, acking each; stops at the first timeout.
    async fn drain_key(store: &TupleStore, stage: &str, key: &str) -> Vec<u64> {
        let mut ids = Vec::new();
        while let Ok((id, _)) = store.take_by_key(stage, key, Duration::from_millis(50)).await {
            store.ack(id).unwrap();
            ids.push(id);
        }
        ids
    }

    /// A keyed item whose lease expires goes to the back of its key's queue; it does not overwrite a newer
    /// item put under the same key while it was in flight.
    #[tokio::test]
    async fn a_requeued_keyed_item_does_not_displace_a_newer_one() {
        let store = TupleStore::transient(500);
        let a = store.put_keyed("s", Arc::from("k"), b("a")).unwrap();
        let (got, _) = store.take_by_key("s", "k", Duration::from_millis(50)).await.unwrap();
        assert_eq!(got, a);
        let newer = store.put_keyed("s", Arc::from("k"), b("b")).unwrap();
        assert_eq!(store.requeue_expired(Duration::ZERO), vec![a]);
        assert_eq!(drain_key(&store, "s", "k").await, vec![newer, a], "both remain, the requeued one at the back");
        assert_eq!(store.depth(Some("s"))[0].depth, 0);
    }

    /// A mirror (or a replay) applying two keyed records under one key keeps both.
    #[tokio::test]
    async fn a_mirror_keeps_two_same_key_records() {
        let store = TupleStore::transient(500);
        store.put_with_id("s", 5, b("x"), Some(Arc::from("k"))).unwrap();
        store.put_with_id("s", 6, b("y"), Some(Arc::from("k"))).unwrap();
        assert_eq!(drain_key(&store, "s", "k").await, vec![5, 6]);
        assert_eq!(store.depth(Some("s"))[0].depth, 0);
    }

    /// Two workers parked on one key are both served, in arrival order; the second does not evict the first.
    #[tokio::test]
    async fn two_waiters_on_one_key_are_both_served() {
        let store = Arc::new(TupleStore::transient(500));
        let (s1, s2) = (Arc::clone(&store), Arc::clone(&store));
        let first = tokio::spawn(async move { s1.take_by_key("s", "k", Duration::from_secs(5)).await });
        let_park().await;
        let second = tokio::spawn(async move { s2.take_by_key("s", "k", Duration::from_secs(5)).await });
        let_park().await;
        let a = store.put_keyed("s", Arc::from("k"), b("a")).unwrap();
        let bb = store.put_keyed("s", Arc::from("k"), b("b")).unwrap();
        let first = first.await.unwrap().expect("the first waiter is served, not evicted");
        let second = second.await.unwrap().expect("the second waiter is served");
        assert_eq!((first.0, second.0), (a, bb), "same-key waiters are served in arrival order");
        assert_eq!(store.depth(Some("s"))[0].depth, 0);
    }

    /// A keyed waiter that times out removes only its own registration: a waiter parked on the same key before it
    /// stays, and is served by the next put.
    #[tokio::test]
    async fn a_timed_out_keyed_waiter_does_not_remove_another() {
        let store = Arc::new(TupleStore::transient(500));
        let (s1, s2) = (Arc::clone(&store), Arc::clone(&store));
        let long = tokio::spawn(async move { s1.take_by_key("s", "k", Duration::from_secs(5)).await });
        let_park().await;
        let short = tokio::spawn(async move { s2.take_by_key("s", "k", Duration::from_millis(20)).await });
        assert!(matches!(short.await.unwrap(), Err(TupleError::Timeout)));
        let id = store.put_keyed("s", Arc::from("k"), b("x")).unwrap();
        let (got, _) = long.await.unwrap().expect("the other waiter stayed parked");
        assert_eq!(got, id);
        assert_eq!(store.depth(Some("s"))[0].depth, 0, "handed over, not stored");
    }

    /// Same-key collisions leave no residue: the depth returns to zero, puts are accepted again at the watermark,
    /// and a timed-out keyed waiter on a key nobody touches again leaves no entry behind.
    #[tokio::test]
    async fn depth_returns_to_zero_after_keyed_collisions() {
        let store = TupleStore::transient(3);
        for i in 0..3 {
            store.put_keyed("s", Arc::from("k"), Bytes::from(format!("{i}"))).unwrap();
        }
        assert_eq!(drain_key(&store, "s", "k").await.len(), 3);
        assert_eq!(store.depth(Some("s"))[0].depth, 0);
        for i in 0..3 {
            store.put_keyed("s", Arc::from(format!("other-{i}")), b("v")).expect("the stage is empty again");
        }
        // A cold key: one timed-out waiter, never touched again.
        assert!(store.take_by_key("cold", "job-1", Duration::from_millis(10)).await.is_err());
        let cold = store.stage("cold");
        assert!(cold.inner.lock().keyed_waiters.is_empty(), "a timed-out waiter's entry is removed");
        assert_eq!(cold.waiters_count.load(Ordering::Relaxed), 0);
    }

    /// A `take_by_key` whose future is dropped (a cancelled task, a gateway handler whose client went away) on a key
    /// nobody touches again leaves no registration and no `waiters` count behind.
    #[tokio::test]
    async fn a_dropped_keyed_take_on_a_cold_key_leaves_nothing() {
        let store = Arc::new(TupleStore::transient(500));
        let s1 = Arc::clone(&store);
        let w = tokio::spawn(async move { s1.take_by_key("s", "job-9", Duration::from_secs(30)).await });
        let_park().await;
        assert_eq!(store.depth(Some("s"))[0].waiters, 1);
        w.abort();
        assert!(w.await.unwrap_err().is_cancelled());
        let st = store.stage("s");
        assert!(st.inner.lock().keyed_waiters.is_empty(), "the dropped take's registration is removed");
        assert_eq!(st.waiters_count.load(Ordering::Relaxed), 0, "and not counted");
    }

    /// R11-11: a worker parked on `take_by_key` counts in the stage's `waiters`, and leaves it when served or
    /// timed out.
    #[tokio::test]
    async fn a_parked_keyed_worker_counts_as_a_waiter() {
        let store = Arc::new(TupleStore::transient(500));
        let s1 = Arc::clone(&store);
        let w = tokio::spawn(async move { s1.take_by_key("s", "k", Duration::from_secs(5)).await });
        let_park().await;
        assert_eq!(store.depth(Some("s"))[0].waiters, 1, "the parked keyed worker is counted");
        store.put_keyed("s", Arc::from("k"), b("x")).unwrap();
        w.await.unwrap().unwrap();
        assert_eq!(store.depth(Some("s"))[0].waiters, 0, "served: no longer counted");
        assert!(store.take_by_key("s", "k", Duration::from_millis(10)).await.is_err());
        assert_eq!(store.depth(Some("s"))[0].waiters, 0, "timed out: no longer counted");
    }

    #[tokio::test]
    async fn take_before_put() {
        let store = Arc::new(TupleStore::transient(500));
        let s2 = Arc::clone(&store);
        let waiter =
            tokio::spawn(async move { s2.take("s", Duration::from_secs(5)).await });
        // Poll until the waiter is parked (structural, not a sleep).
        for _ in 0..100 {
            if store.depth(Some("s")).first().map(|d| d.waiters) == Some(1) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let id = store.put("s", b("x")).unwrap();
        let (got, payload) = waiter.await.unwrap().unwrap();
        assert_eq!(got, id);
        assert_eq!(payload.as_ref(), b"x");
        // No spurious wakeup: queue stayed empty, hot counter incremented.
        assert_eq!(store.depth(Some("s"))[0].depth, 0);
        let states = store.stage_states();
        assert_eq!(states[0].1.hot_total.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn take_timeout() {
        let store = TupleStore::transient(500);
        let r = store.take("empty", Duration::from_millis(50)).await;
        assert!(matches!(r, Err(TupleError::Timeout)));
    }

    #[tokio::test]
    async fn timed_out_waiter_skipped() {
        let store = TupleStore::transient(500);
        let r = store.take("s", Duration::from_millis(20)).await;
        assert!(matches!(r, Err(TupleError::Timeout)));
        // The dead waiter's sender is still queued; put must skip it and store.
        let id = store.put("s", b("x")).unwrap();
        let (got, _) = store.take("s", Duration::from_millis(100)).await.unwrap();
        assert_eq!(got, id);
    }

    #[tokio::test]
    async fn all_waiters_timed_out_item_in_queue() {
        let path = temp_wal("all-timeout");
        let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
        for _ in 0..3 {
            let r = store.take("s", Duration::from_millis(10)).await;
            assert!(matches!(r, Err(TupleError::Timeout)));
        }
        let id = store.put("s", b("x")).unwrap();
        assert_eq!(store.depth(Some("s"))[0].depth, 1);
        let (got, _) = store.take("s", Duration::from_millis(50)).await.unwrap();
        assert_eq!(got, id);
        // Exactly ONE PutRecord despite three dead waiters being skipped.
        let puts = read_wal(&path)
            .iter()
            .filter(|r| matches!(r, Record::Put { .. }))
            .count();
        assert_eq!(puts, 1);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_100() {
        let store = Arc::new(TupleStore::transient(100_000));
        let mut handles = Vec::new();
        for p in 0..10u64 {
            let s = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                for i in 0..100u64 {
                    let payload = Bytes::from(format!("{p}:{i}"));
                    s.put("s", payload).unwrap();
                }
            }));
        }
        let mut consumers = Vec::new();
        for _ in 0..10 {
            let s = Arc::clone(&store);
            consumers.push(tokio::spawn(async move {
                let mut got = Vec::new();
                for _ in 0..100 {
                    let (id, _) = s.take("s", Duration::from_secs(10)).await.unwrap();
                    got.push(id);
                }
                got
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        let mut all: Vec<u64> = Vec::new();
        for c in consumers {
            all.extend(c.await.unwrap());
        }
        // All 1 000 items delivered exactly once.
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 1000);
    }

    #[tokio::test]
    async fn complete_atomic_no_crash_window() {
        let store = TupleStore::transient(500);
        let id = store.put("a", b("payload")).unwrap();
        let (got, payload) = store.take("a", Duration::from_millis(100)).await.unwrap();
        assert_eq!(got, id);
        let next_id = store.complete(id, "b", payload).unwrap();
        assert_ne!(next_id, id);
        // Old stage empty, new stage has the item, nothing in-flight.
        assert_eq!(store.depth(Some("a"))[0].depth, 0);
        assert_eq!(store.depth(Some("b"))[0].depth, 1);
        assert_eq!(store.inflight_count(), 0);
        // Double-complete / double-ack refused.
        assert!(matches!(store.ack(id), Err(TupleError::NotFound)));
    }

    /// Documents the duplicate hazard `complete` eliminates: with separate
    /// put + ack, a crash between the two replays BOTH the old item (taken,
    /// never acked) and the new one — the classic duplicate.
    #[tokio::test]
    async fn separate_put_ack_crash_window_duplicates() {
        let path = temp_wal("crash-window");
        {
            let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
            let id = store.put("a", b("v")).unwrap();
            let (_, payload) = store.take("a", Duration::from_millis(100)).await.unwrap();
            // Non-atomic transition: put(next) succeeded…
            store.put("b", payload).unwrap();
            // …crash before ack(id). (Store dropped without ack.)
            let _ = id;
        }
        let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
        // Old item re-queued AND new item present — 2 live items from 1.
        let total: u32 = store.depth(None).iter().map(|d| d.depth).sum();
        assert_eq!(total, 2);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn wal_records_take() {
        let path = temp_wal("records-take");
        let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
        store.put("s", b("x")).unwrap();
        store.take("s", Duration::from_millis(100)).await.unwrap();
        let recs = read_wal(&path);
        assert!(matches!(recs[0], Record::Put { .. }));
        assert!(matches!(recs[1], Record::Take { .. }));
        // Compaction must NOT drop the unacked (in-flight) item.
        store.compact_now().unwrap();
        let recs = read_wal(&path);
        assert!(recs.iter().any(|r| matches!(r, Record::Put { .. })));
        assert!(recs.iter().any(|r| matches!(r, Record::Take { .. })));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn wal_replay() {
        let path = temp_wal("replay");
        let mut ids = Vec::new();
        {
            let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
            for i in 0..10 {
                ids.push(store.put("s", Bytes::from(format!("item-{i}"))).unwrap());
            }
        }
        let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
        assert_eq!(store.depth(Some("s"))[0].depth, 10);
        // FIFO order and ids survive the restart; next_id is fenced past them.
        for expect in &ids {
            let (id, _) = store.take("s", Duration::from_millis(100)).await.unwrap();
            assert_eq!(id, *expect);
        }
        let fresh = store.put("s", b("new")).unwrap();
        assert!(fresh > *ids.last().unwrap());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn wal_replay_inflight() {
        let path = temp_wal("replay-inflight");
        {
            let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
            for i in 0..5 {
                store.put("s", Bytes::from(format!("{i}"))).unwrap();
            }
            for _ in 0..3 {
                store.take("s", Duration::from_millis(100)).await.unwrap();
            }
            // 3 in-flight (no ack), 2 queued — then crash.
        }
        let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
        // 3 re-queued as abandoned + 2 remaining = 5.
        assert_eq!(store.depth(Some("s"))[0].depth, 5);
        let _ = std::fs::remove_file(&path);
    }

    // ── M13 / WS-G · G-G1b: keyed durability (WAL v2 + replay) ──────────────

    #[tokio::test]
    async fn keyed_item_survives_wal_replay_under_its_key() {
        let path = temp_wal("keyed-replay");
        {
            let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
            store.put_keyed("po", Arc::from("inv-7"), b("matched")).unwrap();
        }
        // Reopen (the WAL is v2 with a keyed Put record) — the item re-queues under its key, so a
        // post-restart take_by_key still rendezvouses (and an unkeyed take does NOT claim it).
        let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
        assert!(store.take("po", Duration::from_millis(50)).await.is_err(),
            "the replayed keyed item is not on the FIFO");
        let (_, payload) = store.take_by_key("po", "inv-7", Duration::from_millis(100)).await.unwrap();
        assert_eq!(payload.as_ref(), &b"matched"[..]);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn acked_keyed_item_does_not_resurrect() {
        let path = temp_wal("keyed-acked");
        {
            let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
            let id = store.put_keyed("po", Arc::from("k"), b("v")).unwrap();
            let (got, _) = store.take_by_key("po", "k", Duration::from_millis(100)).await.unwrap();
            assert_eq!(got, id);
            store.ack(id).unwrap();
        }
        let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
        assert!(store.take_by_key("po", "k", Duration::from_millis(50)).await.is_err(),
            "an acked keyed item must not resurrect on replay");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn v1_wal_replays_on_v2_build() {
        // A WAL stamped with the PREVIOUS format version, holding an (unkeyed) Put, must replay on
        // this v2 build — the rolling-upgrade guarantee. Unkeyed records are byte-identical across
        // versions, so only the header version differs.
        let path = temp_wal("v1-compat");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(WAL_MAGIC);
        bytes.extend_from_slice(&PREV_WAL_VERSION.to_le_bytes());
        Record::Put { id: 1, stage: Arc::from("s"), payload: b("legacy"), key: None }.encode(&mut bytes);
        std::fs::write(&path, &bytes).unwrap();

        let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
        let (id, payload) = store.take("s", Duration::from_millis(100)).await.unwrap();
        assert_eq!((id, payload.as_ref()), (1, &b"legacy"[..]), "v1 WAL replays unchanged");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn wal_compact() {
        let path = temp_wal("compact");
        let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
        let mut ids = Vec::new();
        for i in 0..10 {
            ids.push(store.put("a", Bytes::from(format!("{i}"))).unwrap());
        }
        // Ack 60% via take + complete(terminal ack pattern: take then ack).
        for _ in 0..6 {
            let (id, _) = store.take("a", Duration::from_millis(100)).await.unwrap();
            store.ack(id).unwrap();
        }
        assert!(store.wants_compaction());
        let before = store.wal_bytes();
        store.compact_now().unwrap();
        assert!(store.wal_bytes() < before);
        // Remaining 40% survive a replay of the compacted log.
        drop(store);
        let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
        assert_eq!(store.depth(Some("a"))[0].depth, 4);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn backpressure_error() {
        let store = TupleStore::transient(3);
        for i in 0..3 {
            store.put("s", Bytes::from(format!("{i}"))).unwrap();
        }
        let r = store.put("s", b("overflow"));
        assert!(matches!(r, Err(TupleError::Backpressure { .. })));
        // Depth drops below the watermark → puts succeed again.
        store.take("s", Duration::from_millis(100)).await.unwrap();
        assert!(store.put("s", b("ok")).is_ok());
    }

    #[tokio::test]
    async fn requeue_expired_returns_item() {
        let store = TupleStore::transient(500);
        let id = store.put("s", b("x")).unwrap();
        store.take("s", Duration::from_millis(100)).await.unwrap();
        assert_eq!(store.inflight_count(), 1);
        // Zero timeout: everything in-flight is expired.
        let requeued = store.requeue_expired(Duration::ZERO);
        assert_eq!(requeued, vec![id]);
        assert_eq!(store.inflight_count(), 0);
        let (got, _) = store.take("s", Duration::from_millis(100)).await.unwrap();
        assert_eq!(got, id);
    }

    #[tokio::test]
    async fn wal_chunk_pagination_and_epoch_reset() {
        let path = temp_wal("chunk");
        let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
        for i in 0..10 {
            store.put("s", Bytes::from(format!("item-{i}"))).unwrap();
        }
        // Page through with a 3-entry budget; records must round-trip.
        let (epoch, head) = store.wal_position();
        assert_eq!(epoch, 0);
        let mut offset = 0u64;
        let mut seen = 0usize;
        loop {
            let c = store
                .wal_read_chunk(epoch, offset, 3, 1 << 20)
                .unwrap()
                .unwrap();
            assert_eq!(c.epoch, epoch);
            seen += decode_records(&c.raw).len();
            offset = c.next_offset;
            if c.done {
                break;
            }
        }
        assert_eq!(seen, 10);
        assert_eq!(offset, head);
        // Stale epoch (e.g. after a compaction) → no data, restart cursor.
        for _ in 0..6 {
            let (id, _) = store.take("s", Duration::from_millis(50)).await.unwrap();
            store.ack(id).unwrap();
        }
        store.compact_now().unwrap();
        let c = store
            .wal_read_chunk(epoch, offset, 3, 1 << 20)
            .unwrap()
            .unwrap();
        assert_eq!(c.epoch, epoch + 1);
        assert!(c.raw.is_empty());
        assert_eq!(c.next_offset, 0);
        let _ = std::fs::remove_file(&path);
    }

    /// Run-17 improvement target (Evolvability): a WAL stamped with a FUTURE
    /// format version must be refused untouched — never silently truncated.
    #[tokio::test]
    async fn wal_future_version_refused_untouched() {
        let path = temp_wal("future-version");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(WAL_MAGIC);
        bytes.extend_from_slice(&99u16.to_le_bytes());
        bytes.extend_from_slice(b"opaque future-format payload");
        std::fs::write(&path, &bytes).unwrap();

        let err = match TupleStore::persistent(&path, 10_000, 500) {
            Ok(_) => panic!("future WAL version must refuse to open"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("v99"), "error names the version: {err}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "refusal must leave the file byte-identical"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn wal_foreign_file_refused_untouched() {
        let path = temp_wal("foreign");
        std::fs::write(&path, b"definitely not a tuple-space wal").unwrap();
        let err = match TupleStore::persistent(&path, 10_000, 500) {
            Ok(_) => panic!("non-WAL file must refuse to open"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("magic"), "error names the cause: {err}");
        assert_eq!(
            std::fs::read(&path).unwrap().as_slice(),
            b"definitely not a tuple-space wal",
            "refusal must leave the file byte-identical"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// M2 Run-18 probe (Evolvability): a header torn mid-write — magic intact
    /// but version bytes missing — must also be refused untouched, not
    /// stamped over or truncated.
    #[tokio::test]
    async fn wal_torn_header_refused_untouched() {
        let path = temp_wal("torn-header");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(WAL_MAGIC);
        bytes.push(1); // first byte of the version, second missing
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            TupleStore::persistent(&path, 10_000, 500).is_err(),
            "7-byte torn header must refuse to open"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "refusal must leave the file byte-identical"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn wal_header_written_and_survives_compaction() {
        let path = temp_wal("header");
        {
            let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
            store.put("s", b("x")).unwrap();
            store.compact_now().unwrap();
        }
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..6], WAL_MAGIC);
        assert_eq!(u16::from_le_bytes(bytes[6..8].try_into().unwrap()), WAL_VERSION);
        // And the post-compaction file replays.
        let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
        assert_eq!(store.depth(Some("s"))[0].depth, 1);
        let _ = std::fs::remove_file(&path);
    }

    /// ER6-T, the fence half: `fence_past` below the store's own base (ER3) changes nothing and logs
    /// nothing; above it, the fence is persisted as a lone `Take` that survives a compaction and a
    /// reopen and changes no item's liveness.
    #[tokio::test]
    async fn fence_past_persists_without_touching_liveness() {
        let path = temp_wal("fence-past");
        let fence = {
            let store = TupleStore::persistent(&path, 1, 500).unwrap();
            let first = store.put("s", b("x")).unwrap();
            let len = std::fs::metadata(&path).unwrap().len();
            assert!(!store.fence_past(1).unwrap(), "below the store's own base: no move");
            assert!(!store.fence_past(first).unwrap());
            assert_eq!(std::fs::metadata(&path).unwrap().len(), len, "and nothing logged");
            let fence = first + 50;
            assert!(store.fence_past(fence).unwrap());
            fence
        };
        for round in ["reopen", "compaction"] {
            let store = TupleStore::persistent(&path, 1, 500).unwrap();
            assert_eq!(store.depth(Some("s")).first().map_or(0, |d| d.depth), 1, "{round}: the item stays");
            store.compact_now().unwrap();
        }
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        let next = store.put("s", b("y")).unwrap();
        assert!(next >= fence, "the reopened store issued {next}, below its fence {fence}");
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn put_with_id_fences_next_id() {
        let store = TupleStore::transient(500);
        // Relative to the store's first id: a fresh id space does not start at 0 (ER3).
        let first = store.put("s", b("first")).unwrap();
        let mirrored = first + 41;
        store.put_with_id("s", mirrored, b("mirrored"), None).unwrap();
        let fresh = store.put("s", b("new")).unwrap();
        assert!(fresh > mirrored, "next_id not fenced past replicated id");
        assert!(store.remove_queued("s", mirrored));
        assert!(!store.remove_queued("s", mirrored), "double remove must be a no-op");
    }

    /// M2 Run-17 falsification probe (Semantic Correctness): a crash mid-append
    /// leaves a torn record at the WAL tail. Claimed invariant: replay drops
    /// the torn tail, recovers every complete record, and the log remains
    /// appendable. Probe truncates at EVERY byte offset inside the final
    /// record, not just one convenient point.
    #[tokio::test]
    async fn wal_torn_tail_recovery() {
        let path = temp_wal("torn-tail");
        {
            let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
            for i in 0..5 {
                store.put("s", Bytes::from(format!("item-{i}"))).unwrap();
            }
            let (id, _) = store.take("s", Duration::from_millis(50)).await.unwrap();
            store.ack(id).unwrap();
        }
        let full = std::fs::read(&path).unwrap();
        // Find the start of the last record so we can tear inside it.
        let recs = read_wal(&path);
        assert!(recs.len() >= 7); // 5 puts + take + ack
        let mut offset = WAL_HEADER_LEN as usize;
        for _ in 0..recs.len() - 1 {
            let (_, n) = Record::decode(&full[offset..]).unwrap();
            offset += n;
        }
        for cut in offset + 1..full.len() {
            std::fs::write(&path, &full[..cut]).unwrap();
            let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
            // Last record was the ack: torn ack ⇒ item re-queues (5 live);
            // the 6 preceding records (5 puts + take) must all survive,
            // with the taken-unacked item re-queued as abandoned ⇒ depth 5.
            assert_eq!(
                store.depth(Some("s"))[0].depth,
                5,
                "torn tail at byte {cut} corrupted recovery"
            );
            // Log must remain appendable and re-replayable after the tear.
            store.put("s", b("post-tear")).unwrap();
            drop(store);
            let store = TupleStore::persistent(&path, 10_000, 500).unwrap();
            assert_eq!(store.depth(Some("s"))[0].depth, 6);
        }
        let _ = std::fs::remove_file(&path);
    }

    /// Row C (post-360 hardening): a failed append leaves part of a frame on disk. The writer used
    /// to carry on, so the next acknowledged put landed *behind* the torn frame, and the next open
    /// read the torn frame's length over it, called the lot a torn tail and truncated it — an
    /// acknowledged put silently gone. Every put that returned `Ok` must survive a reopen.
    #[tokio::test]
    async fn a_failed_append_never_strands_a_later_acknowledged_put() {
        let path = temp_wal("failed-append");
        let mut acked = Vec::new();
        {
            let store = TupleStore::persistent(&path, 1, 500).unwrap();
            acked.push(store.put("s", b("before")).unwrap());
            store.wal.as_ref().unwrap().fault.store(true, Ordering::SeqCst);
            assert!(store.put("s", Bytes::from(vec![7u8; 200])).is_err(), "the injected failure is reported");
            // After the failure, a put either is refused or survives the reopen — never both Ok and lost.
            if let Ok(id) = store.put("s", b("after")) {
                acked.push(id);
            }
        }
        let store = TupleStore::persistent(&path, 1, 500)
            .expect("a torn final frame is a crash's signature: truncated, not refused");
        let mut got = Vec::new();
        while let Ok((id, _)) = store.take("s", Duration::from_millis(20)).await {
            got.push(id);
        }
        assert_eq!(got, acked, "every acknowledged put survives the reopen");
        let _ = std::fs::remove_file(&path);
    }

    /// Row C: a complete frame that does not decode, with records after it, is corruption — not a
    /// crash. The open used to stop there and truncate, discarding every later record silently. It
    /// must refuse by name and leave the file untouched.
    #[tokio::test]
    async fn a_corrupt_middle_record_refuses_the_open_and_leaves_the_file() {
        let path = temp_wal("corrupt-middle");
        {
            let store = TupleStore::persistent(&path, 1, 500).unwrap();
            store.put("s", b("first")).unwrap();
        }
        let mut bytes = std::fs::read(&path).unwrap();
        // A whole frame of an unknown kind, then a good record after it.
        bytes.push(99);
        bytes.extend_from_slice(&8u32.to_le_bytes());
        bytes.extend_from_slice(&[0xAB; 8]);
        Record::Put { id: 1, stage: Arc::from("s"), payload: b("second"), key: None }.encode(&mut bytes);
        std::fs::write(&path, &bytes).unwrap();
        let err = TupleStore::persistent(&path, 1, 500).err().expect("a corrupt middle record refuses the open");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains(&path.display().to_string()), "the refusal names the file: {err}");
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "a refused open leaves the file as it was");
        let _ = std::fs::remove_file(&path);
    }

    /// Row C: a write appends to the WAL and *then* applies to memory. A compaction that rewrote the
    /// log from memory, landing between the two, wrote a log without the record — and the put was
    /// still acknowledged, so a crash after that compaction lost it. The acknowledged put must
    /// survive a reopen whatever compaction ran in that window.
    #[tokio::test]
    async fn a_compaction_between_append_and_apply_keeps_the_acknowledged_put() {
        let path = temp_wal("compact-window");
        let id = {
            let store = TupleStore::persistent(&path, 1, 500).unwrap();
            COMPACT_BETWEEN.with(|c| c.set(true));
            let id = store.put("s", b("in-the-window")).unwrap();
            assert!(!COMPACT_BETWEEN.with(|c| c.get()), "the compaction ran in the window");
            id
        };
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        let got = store.take("s", Duration::from_millis(50)).await.map(|(id, _)| id);
        assert_eq!(got.ok(), Some(id), "the acknowledged put survives the compaction and the reopen");
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 7: compaction drops acked records, and open fenced `next_id` from the
    /// highest id *left*, so a restart reissued an acknowledged id — and a late duplicate ack for the
    /// old item acked a different one.
    #[tokio::test]
    async fn a_restart_never_reuses_an_acked_id() {
        let path = temp_wal("id-high-water");
        let first;
        {
            let store = TupleStore::persistent(&path, 1, 500).unwrap();
            // Relative to the first id: a fresh WAL's id space does not start at 0 (ER3).
            first = store.put("s", b("0")).unwrap();
            for i in 1..3 {
                store.put("s", Bytes::from(format!("{i}"))).unwrap();
            }
            for _ in 0..3 {
                let (id, _) = store.take("s", Duration::from_millis(50)).await.unwrap();
                store.ack(id).unwrap();
            }
            store.compact_now().unwrap();
        }
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        assert_eq!(store.put("s", b("next")).unwrap(), first + 3, "the next id is past every id the log ever held");
        let _ = std::fs::remove_file(&path);
    }

    /// ER3 (R4-01): a fresh id space started at 0, so a transient store recreated in the same
    /// namespace reissued the ids of its previous life, and a destination keyed on the id answered
    /// `Replayed` for new work. Two transient stores back to back must not overlap.
    #[tokio::test]
    async fn fresh_id_spaces_do_not_overlap() {
        let first = TupleStore::transient(500);
        let old: Vec<u64> = (0..3).map(|i| first.put("s", Bytes::from(format!("{i}"))).unwrap()).collect();
        drop(first);
        let second = TupleStore::transient(500);
        let new = second.put("s", b("new")).unwrap();
        assert!(new > *old.iter().max().unwrap(), "the restarted store reissued an id: old {old:?}, new {new}");
        // And a WAL with no history starts fresh too: a deleted WAL does not recycle ids.
        let path = temp_wal("fresh-after-transient");
        let walled = TupleStore::persistent(&path, 1, 500).unwrap();
        let from_wal = walled.put("s", b("wal")).unwrap();
        assert!(from_wal > new, "a WAL with no history reissued an id: {from_wal} after {new}");
        drop(walled);
        let _ = std::fs::remove_file(&path);
    }

    /// ER3: the fresh base is applied only where a store has no id history. A WAL that names ids
    /// keeps its fence at exactly `max_id + 1`, whatever the clock says, so a redelivery after a
    /// restart still carries the same id. A control: passes before and after the fix.
    #[tokio::test]
    async fn a_wal_with_history_keeps_its_fence() {
        let path = temp_wal("history-fence");
        let last;
        {
            let store = TupleStore::persistent(&path, 1, 500).unwrap();
            store.put("s", b("a")).unwrap();
            last = store.put("s", b("b")).unwrap();
        }
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        assert_eq!(store.put("s", b("c")).unwrap(), last + 1, "the fence is the log's, not the clock's");
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    /// ER3: ids stay below 2^53, so the gateway's JSON numbers are exact in TypeScript. A pin: an id
    /// space at 0 passes it too.
    #[tokio::test]
    async fn ids_stay_below_two_to_the_53() {
        let store = TupleStore::transient(500);
        let id = store.put("s", b("x")).unwrap();
        assert!(id < 1u64 << 53, "id {id} is not exact as a JSON number");
    }

    /// ER3, the clock half (review of f4972912): every restart test above runs in one process, so the
    /// in-process mark alone passes them. A fresh store's first id is at or above the wall clock's
    /// base, so a new process — a fresh mark — cannot start below it.
    #[tokio::test]
    async fn a_fresh_store_starts_at_or_above_the_wall_clock() {
        let before = mycelium::sim_seam::wall_now_ms() << 10;
        let id = TupleStore::transient(500).put("s", b("x")).unwrap();
        assert!(id >= before, "the first id {id} is below the clock's base {before}");
    }

    /// ER3, the clock half: a restart in a **new process** — modelled by a mark of its own at 0, not
    /// by resetting the shared static, which other tests in this binary use concurrently — starts
    /// above the old store's ids once the clock has moved past them (the named limit: within
    /// *N*/1024 ms they can overlap).
    #[tokio::test]
    async fn a_new_process_starts_above_the_old_stores_ids() {
        let old = TupleStore::transient(500);
        let old_ids: Vec<u64> = (0..3).map(|_| old.put("s", b("x")).unwrap()).collect();
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

    /// #597 review, finding 3: a valid kind byte followed by zeros is what a crash leaves when the
    /// filesystem zero-extends a partial append — a torn tail, not corruption.
    #[tokio::test]
    async fn a_valid_kind_followed_by_zeros_is_a_torn_tail() {
        let path = temp_wal("zero-tail");
        {
            let store = TupleStore::persistent(&path, 1, 500).unwrap();
            store.put("s", b("kept")).unwrap();
        }
        let mut bytes = std::fs::read(&path).unwrap();
        let good = bytes.len();
        bytes.push(REC_PUT);
        bytes.extend_from_slice(&[0u8; 12]);
        std::fs::write(&path, &bytes).unwrap();
        let store = TupleStore::persistent(&path, 1, 500).expect("a zero-filled tail is torn, not corrupt");
        assert_eq!(store.depth(Some("s"))[0].depth, 1);
        assert_eq!(std::fs::metadata(&path).unwrap().len() as usize, good, "the torn tail is truncated");
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 6: a whole frame of a known kind whose body does not decode, with a record
    /// after it, refuses the open (the unknown-kind case is the other test).
    #[tokio::test]
    async fn a_known_kind_whose_body_does_not_decode_refuses_the_open() {
        let path = temp_wal("bad-body");
        {
            let store = TupleStore::persistent(&path, 1, 500).unwrap();
            store.put("s", b("first")).unwrap();
        }
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.push(REC_PUT);
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&[1, 2, 3]);
        Record::Put { id: 1, stage: Arc::from("s"), payload: b("second"), key: None }.encode(&mut bytes);
        std::fs::write(&path, &bytes).unwrap();
        let err = TupleStore::persistent(&path, 1, 500).err().expect("refused");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 6: the repair path end to end — a failed append, the refusal after it, a
    /// compaction, appends resuming, and a reopen holding every acknowledged put.
    #[tokio::test]
    async fn a_compaction_repairs_a_poisoned_wal_and_appends_resume() {
        let path = temp_wal("repair");
        let (a, c) = {
            let store = TupleStore::persistent(&path, 1, 500).unwrap();
            let a = store.put("s", b("a")).unwrap();
            store.inject_append_fault();
            assert!(store.put("s", b("lost")).is_err());
            assert!(store.put("s", b("refused")).is_err(), "poisoned");
            assert!(store.wants_compaction(), "a poisoned writer asks for the repair");
            store.compact_now().unwrap();
            assert!(!store.wal_poisoned());
            let c = store.put("s", b("c")).unwrap();
            (a, c)
        };
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        let mut got = Vec::new();
        while let Ok((id, _)) = store.take("s", Duration::from_millis(20)).await {
            got.push(id);
        }
        assert_eq!(got, vec![a, c]);
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 6: a memory-first write (an ack) racing a compaction — the compaction runs
    /// after the in-memory removal and before the WAL append — is not lost: the ack lands in the
    /// rewritten log.
    #[tokio::test]
    async fn an_ack_racing_a_compaction_is_kept() {
        let path = temp_wal("ack-window");
        {
            let store = TupleStore::persistent(&path, 1, 500).unwrap();
            store.put("s", b("x")).unwrap();
            let (id, _) = store.take("s", Duration::from_millis(50)).await.unwrap();
            COMPACT_BETWEEN.with(|c| c.set(true));
            store.ack(id).unwrap();
            assert!(!COMPACT_BETWEEN.with(|c| c.get()), "the compaction ran in the window");
        }
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        assert!(store.depth(Some("s")).iter().all(|d| d.depth == 0), "the acked item does not come back");
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 1: a mirror's `remove_queued` dropped the append's result and counted the
    /// ack anyway, so the writer's accounting claimed a terminal record the log never received.
    #[tokio::test]
    async fn a_refused_mirror_ack_is_not_counted() {
        let path = temp_wal("mirror-ack");
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        store.put_with_id("s", 5, b("x"), None).unwrap();
        store.inject_append_fault();
        assert!(store.remove_queued("s", 5));
        assert_eq!(store.wal_acked_count(), 0, "a refused ack record is not counted");
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 4: a repair that keeps failing reread the whole log on every tick. It
    /// backs off.
    #[tokio::test]
    async fn a_failing_repair_backs_off() {
        let path = temp_wal("repair-backoff");
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        store.inject_append_fault();
        assert!(store.put("s", b("x")).is_err());
        store.set_compact_fault(true);
        let mut attempts = 0;
        for _ in 0..20 {
            if store.wants_compaction() {
                attempts += 1;
                assert!(store.compact_now().is_err());
            }
        }
        assert!(attempts <= 5, "{attempts} attempts in 20 ticks");
        store.set_compact_fault(false);
        let mut repaired = false;
        for _ in 0..200 {
            store.maintenance_tick(false).ok();
            if !store.wal_poisoned() {
                repaired = true;
                break;
            }
        }
        assert!(repaired, "the back-off ends: a later tick repairs");
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 4: the maintenance tick returned at a failed sync before it reached the
    /// repair, so a disk that fails syncs kept the writer poisoned.
    #[tokio::test]
    async fn the_repair_runs_even_when_the_sync_fails() {
        let path = temp_wal("repair-sync-fails");
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        store.inject_append_fault();
        assert!(store.put("s", b("x")).is_err());
        store.set_sync_fault(true);
        assert!(store.maintenance_tick(true).is_err(), "the sync failure is still reported");
        assert!(!store.wal_poisoned(), "and the repair ran");
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 5: the compaction temp file was named by replacing the WAL's extension, so
    /// two WALs sharing a stem (`x.a`, `x.b`) shared one temp path outside either's ownership lock.
    #[tokio::test]
    async fn the_compaction_temp_file_is_named_after_the_whole_wal_file() {
        let dir = std::env::temp_dir().join(format!("mts-stem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Occupy the stem-derived name: a directory there makes a temp file of that name impossible.
        std::fs::create_dir_all(dir.join("x.compact")).unwrap();
        let store = TupleStore::persistent(&dir.join("x.a"), 1, 500).unwrap();
        store.put("s", b("x")).unwrap();
        store.compact_now().expect("the temp path is the WAL's own, not its stem's");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #597 re-review, finding 5: a compaction failing *after* its rename left the writer appending to
    /// the old, unlinked file — every later acknowledged record gone at restart — and unpoisoned.
    #[tokio::test]
    async fn a_failure_after_the_rename_poisons_the_writer() {
        let path = temp_wal("post-rename");
        let (a, c) = {
            let store = TupleStore::persistent(&path, 1, 500).unwrap();
            let a = store.put("s", b("a")).unwrap();
            store.set_post_rename_fault(true);
            assert!(store.compact_now().is_err());
            assert!(store.wal_poisoned(), "an error after the rename poisons the writer");
            assert!(store.put("s", b("refused")).is_err());
            store.set_post_rename_fault(false);
            store.compact_now().unwrap();
            let c = store.put("s", b("c")).unwrap();
            (a, c)
        };
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        let mut got = Vec::new();
        while let Ok((id, _)) = store.take("s", Duration::from_millis(20)).await {
            got.push(id);
        }
        assert_eq!(got, vec![a, c]);
        let _ = std::fs::remove_file(&path);
    }

    /// #597 re-review, finding 4: a compaction failing on an *unpoisoned* writer was retried every
    /// tick, rereading the whole log each time. It backs off too.
    #[tokio::test]
    async fn a_failing_compaction_backs_off_even_when_not_poisoned() {
        let path = temp_wal("unpoisoned-backoff");
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        for _ in 0..2 {
            store.put("s", b("x")).unwrap();
            let (id, _) = store.take("s", Duration::from_millis(50)).await.unwrap();
            store.ack(id).unwrap();
        }
        store.set_compact_fault(true);
        let mut attempts = 0;
        for _ in 0..20 {
            if store.wants_compaction() {
                attempts += 1;
                assert!(store.compact_now().is_err());
            }
        }
        assert!(attempts <= 5, "{attempts} attempts in 20 ticks");
        let _ = std::fs::remove_file(&path);
    }

    /// #597 re-review, finding 4: the back-off is capped near ten seconds of ticks (200 ms each), not
    /// left to grow to a minute of refused appends.
    #[tokio::test]
    async fn the_repair_back_off_is_capped_near_ten_seconds() {
        let path = temp_wal("backoff-cap");
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        store.inject_append_fault();
        assert!(store.put("s", b("x")).is_err());
        store.set_compact_fault(true);
        let mut last = None;
        let mut widest = 0;
        for tick in 0..600u32 {
            if store.wants_compaction() {
                let _ = store.compact_now();
                if let Some(prev) = last {
                    widest = widest.max(tick - prev);
                }
                last = Some(tick);
            }
        }
        assert!(widest <= 51, "{widest} ticks between repair attempts");
        let _ = std::fs::remove_file(&path);
    }

    /// #597 re-review, finding 7: the id high-water mark across two compactions and restarts.
    #[tokio::test]
    async fn the_high_water_mark_survives_two_compactions_and_a_restart() {
        let path = temp_wal("hw-twice");
        let mut first = None;
        {
            let store = TupleStore::persistent(&path, 1, 500).unwrap();
            for _ in 0..3 {
                let put = store.put("s", b("x")).unwrap();
                // Relative to the first id: a fresh WAL's id space does not start at 0 (ER3).
                first.get_or_insert(put);
                let (id, _) = store.take("s", Duration::from_millis(50)).await.unwrap();
                store.ack(id).unwrap();
            }
            store.compact_now().unwrap();
        }
        {
            let store = TupleStore::persistent(&path, 1, 500).unwrap();
            let first = first.unwrap();
            assert_eq!(store.put("s", b("y")).unwrap(), first + 3);
            let (id, _) = store.take("s", Duration::from_millis(50)).await.unwrap();
            store.ack(id).unwrap();
            store.compact_now().unwrap();
            store.compact_now().unwrap();
        }
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        assert_eq!(store.put("s", b("z")).unwrap(), first.unwrap() + 4, "past both compactions and both restarts");
        let _ = std::fs::remove_file(&path);
    }

    /// Row C: one owner per WAL file. Two stores on one path used to replay, append and compact the
    /// same file, each over the other.
    #[tokio::test]
    async fn a_second_owner_of_the_wal_is_refused() {
        let path = temp_wal("second-owner");
        let first = TupleStore::persistent(&path, 1, 500).unwrap();
        let err = TupleStore::persistent(&path, 1, 500).err().expect("a second owner is refused");
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock, "{err}");
        drop(first);
        TupleStore::persistent(&path, 1, 500).expect("the lock is released with its owner");
        let _ = std::fs::remove_file(&path);
    }

    /// Row C: compaction installs its file the way the core installs a snapshot — the temp file's
    /// bytes synced, renamed into place, then the **directory** synced, without which the rename
    /// (and so the compaction) may not survive a power loss.
    #[tokio::test]
    async fn compaction_syncs_the_directory_after_the_rename() {
        let path = temp_wal("compact-durable");
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        store.put("s", b("x")).unwrap();
        FS_TRACE.with(|t| t.borrow_mut().clear());
        store.compact_now().unwrap();
        let trace = FS_TRACE.with(|t| t.borrow().clone());
        let pos = |op: &str| trace.iter().position(|o| *o == op).unwrap_or_else(|| panic!("no {op} in {trace:?}"));
        assert!(pos("tmp.write") < pos("tmp.sync"), "{trace:?}");
        assert!(pos("tmp.sync") < pos("rename"), "{trace:?}");
        assert!(pos("rename") < pos("dir.sync"), "the directory sync makes the rename durable: {trace:?}");
        let _ = std::fs::remove_file(&path);
    }

    /// R6 of `docs/plans/external-review-2026-10-31.md` (the review's F7): a fresh WAL's directory
    /// entry is synced before its first record is synced (the checkpoint), and each directory `open`
    /// creates is synced in the one above it. On the unrepaired code neither sync runs. The
    /// power-loss property itself is not observable without a filesystem fault adapter; this pins
    /// the order.
    #[tokio::test]
    async fn a_fresh_wal_syncs_its_directories_before_the_first_record() {
        let root = std::env::temp_dir().join(format!("mts-r6-{}-{}", std::process::id(), fastrand_tag()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("a").join("wal.log");
        FS_TRACE.with(|t| t.borrow_mut().clear());
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        store.put("s", b("x")).unwrap();
        // Appends land in the page cache; the checkpoint is where a record becomes durable.
        store.checkpoint_if_due(true).unwrap();
        let trace = FS_TRACE.with(|t| t.borrow().clone());
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

    /// #718's review, finding 3: reopening an existing WAL syncs its directory again, as the
    /// journal does — it covers a file an earlier open created whose sync failed or that an earlier
    /// version never synced. On the code before the fix a reopen syncs nothing.
    #[tokio::test]
    async fn reopening_a_wal_syncs_its_directory() {
        let root = std::env::temp_dir().join(format!("mts-r6-reopen-{}-{}", std::process::id(), fastrand_tag()));
        let path = root.join("a").join("wal.log");
        drop(TupleStore::persistent(&path, 1, 500).unwrap());
        FS_DIRS.with(|t| t.borrow_mut().clear());
        let store = TupleStore::persistent(&path, 1, 500).unwrap();
        let dirs = FS_DIRS.with(|t| t.borrow().clone());
        let under: Vec<_> = dirs.iter().filter(|(_, p)| p.starts_with(&root)).cloned().collect();
        assert_eq!(
            under,
            [("open.mkdir.sync", root.clone()), ("open.parent.sync", root.join("a"))],
            "`a`'s entry in the root, then the file's in `a`, on a reopen: {dirs:?}"
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn fastrand_tag() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    }

    /// M2 Run-17 falsification probe (Observability): the monitoring counters
    /// must stay exactly consistent through a chaotic lifecycle — parked-take
    /// timeouts, expiry re-queue, redelivery, mixed ack/complete. Claimed
    /// exact: depth, inflight, waiters return to 0 at quiescence;
    /// hot_total ≤ take_total; put_total counts puts+completes only (not
    /// re-queues).
    #[tokio::test]
    async fn metrics_accounting_identity() {
        let store = TupleStore::transient(500);
        // 3 waiters time out (waiters_count must come back down).
        for _ in 0..3 {
            let _ = store.take("s", Duration::from_millis(10)).await;
        }
        // 8 puts; take 4; ack 2, complete 2 onto stage "t".
        for i in 0..8 {
            store.put("s", Bytes::from(format!("{i}"))).unwrap();
        }
        let mut taken = Vec::new();
        for _ in 0..4 {
            taken.push(store.take("s", Duration::from_millis(100)).await.unwrap());
        }
        store.ack(taken[0].0).unwrap();
        store.ack(taken[1].0).unwrap();
        store.complete(taken[2].0, "t", b("x")).unwrap();
        store.complete(taken[3].0, "t", b("y")).unwrap();
        // Force-expire nothing (no inflight left on "s"); drain everything.
        assert_eq!(store.requeue_expired(Duration::from_secs(999)).len(), 0);
        for _ in 0..4 {
            let (id, _) = store.take("s", Duration::from_millis(100)).await.unwrap();
            store.ack(id).unwrap();
        }
        for _ in 0..2 {
            let (id, _) = store.take("t", Duration::from_millis(100)).await.unwrap();
            store.ack(id).unwrap();
        }
        // ER7 (R11-10): parked successful takers, plain and keyed, and a requeue handed to a parked
        // waiter, on stages "p", "k" and "r". Each hand-off is one take.
        let store = Arc::new(store);
        let s = Arc::clone(&store);
        let w = tokio::spawn(async move { s.take("p", Duration::from_secs(5)).await });
        for _ in 0..20 { tokio::task::yield_now().await; }
        store.put("p", b("x")).unwrap();
        let (id, _) = w.await.unwrap().unwrap();
        store.ack(id).unwrap();
        let s = Arc::clone(&store);
        let w = tokio::spawn(async move { s.take_by_key("k", "c", Duration::from_secs(5)).await });
        for _ in 0..20 { tokio::task::yield_now().await; }
        store.put_keyed("k", Arc::from("c"), b("x")).unwrap();
        let (id, _) = w.await.unwrap().unwrap();
        store.ack(id).unwrap();
        store.put("r", b("x")).unwrap();
        let (id, _) = store.take("r", Duration::from_millis(100)).await.unwrap();
        let s = Arc::clone(&store);
        let w = tokio::spawn(async move { s.take("r", Duration::from_secs(5)).await });
        for _ in 0..20 { tokio::task::yield_now().await; }
        assert_eq!(store.requeue_expired(Duration::ZERO), vec![id]);
        let (id, _) = w.await.unwrap().unwrap();
        store.ack(id).unwrap();
        // Quiescence: exact zeros everywhere.
        assert_eq!(store.inflight_count(), 0);
        for m in store.metrics_snapshot() {
            assert_eq!(m.depth, 0, "stage {} depth nonzero", m.stage);
            assert_eq!(m.inflight, 0, "stage {} inflight nonzero", m.stage);
            assert_eq!(m.waiters, 0, "stage {} waiters leaked", m.stage);
            assert!(m.hot_total <= m.take_total);
            match m.stage.as_ref() {
                "s" => {
                    assert_eq!(m.put_total, 8);
                    assert_eq!(m.take_total, 8);
                }
                "t" => {
                    assert_eq!(m.put_total, 2); // the two completes
                    assert_eq!(m.take_total, 2);
                }
                "p" | "k" => {
                    assert_eq!((m.put_total, m.take_total, m.hot_total), (1, 1, 1), "stage {}", m.stage);
                }
                "r" => {
                    // One put, two deliveries (the take and the redelivery to the parked waiter).
                    assert_eq!((m.put_total, m.take_total), (1, 2));
                }
                other => panic!("unexpected stage {other}"),
            }
        }
    }

    /// M2 Run-17 deep-dive evidence (Performance, #15): hot-path and
    /// store-path throughput smoke. Ignored by default — run explicitly:
    /// `cargo test -p mycelium-tuple-space --release wal_throughput_smoke -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "perf smoke; run explicitly with --ignored --nocapture"]
    async fn wal_throughput_smoke() {
        let n: u64 = 50_000;
        // Transient store-path: put then take, serially interleaved.
        let store = TupleStore::transient(u32::MAX);
        let t0 = std::time::Instant::now();
        for i in 0..n {
            store.put("s", Bytes::from(i.to_le_bytes().to_vec())).unwrap();
            store.take("s", Duration::from_millis(10)).await.unwrap();
        }
        let transient_pairs_s = n as f64 / t0.elapsed().as_secs_f64();
        // WAL-backed store-path (page-cache appends, background fsync model).
        let path = temp_wal("throughput");
        let store = TupleStore::persistent(&path, 10_000, u32::MAX).unwrap();
        let t0 = std::time::Instant::now();
        for i in 0..n {
            store.put("s", Bytes::from(i.to_le_bytes().to_vec())).unwrap();
            let (id, _) = store.take("s", Duration::from_millis(10)).await.unwrap();
            store.ack(id).unwrap();
        }
        let wal_pairs_s = n as f64 / t0.elapsed().as_secs_f64();
        println!("transient: {transient_pairs_s:.0} put/take pairs/s");
        println!("wal:       {wal_pairs_s:.0} put/take/ack cycles/s");
        // Very loose floors — this is a smoke alarm, not a benchmark.
        assert!(transient_pairs_s > 20_000.0);
        assert!(wal_pairs_s > 5_000.0);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn complete_unknown_id_refused() {
        let store = TupleStore::transient(500);
        assert!(matches!(
            store.complete(99, "b", b("x")),
            Err(TupleError::NotFound)
        ));
    }

    // ── Counter-invariant property test (Run-17 improvement target #2) ──────
    //
    // Two consecutive M2 runs found concurrency-accounting defects (Run 16:
    // listener registration race; Run 17: waiters_count double-decrement).
    // This is the structural response: a model-based test that replays
    // arbitrary operation sequences against a reference model and checks
    // every monitoring counter after EVERY step. The Run-17 underflow is
    // re-found by this test in seconds if the fix regresses (verified by
    // temporarily reverting the fix: fails at the first timeout+put pair).

    mod counter_model {
        use super::*;
        use proptest::prelude::*;
        use std::collections::HashMap as StdHashMap;

        #[derive(Debug, Clone)]
        pub(super) enum Op {
            Put(u8),
            TakeReady(u8),
            TakeEmptyTimeout(u8),
            AckOldest,
            CompleteOldest(u8),
            RequeueAll,
        }

        fn op_strategy() -> impl Strategy<Value = Op> {
            prop_oneof![
                3 => (0u8..3).prop_map(Op::Put),
                3 => (0u8..3).prop_map(Op::TakeReady),
                2 => (0u8..3).prop_map(Op::TakeEmptyTimeout),
                2 => Just(Op::AckOldest),
                2 => (0u8..3).prop_map(Op::CompleteOldest),
                1 => Just(Op::RequeueAll),
            ]
        }

        #[derive(Default)]
        struct Model {
            queues: StdHashMap<u8, VecDeque<u64>>,
            /// Claim order, oldest first.
            inflight: Vec<(u64, u8)>,
            /// Timed-out waiters whose dead senders are still queued.
            dead_waiters: StdHashMap<u8, u32>,
        }

        fn stage_name(s: u8) -> String {
            format!("st-{s}")
        }

        fn check(store: &TupleStore, model: &Model, step: usize) {
            let by_stage = store.inflight_by_stage();
            let model_inflight: usize = model.inflight.len();
            let store_inflight: u32 = by_stage.values().sum();
            assert_eq!(
                store_inflight as usize, model_inflight,
                "step {step}: inflight mismatch"
            );
            for m in store.metrics_snapshot() {
                let s: u8 = m.stage.trim_start_matches("st-").parse().unwrap();
                let want_depth =
                    model.queues.get(&s).map_or(0, |q| q.len()) as u32;
                let want_waiters =
                    model.dead_waiters.get(&s).copied().unwrap_or(0);
                assert_eq!(m.depth, want_depth, "step {step}: depth[{s}]");
                assert_eq!(m.waiters, want_waiters, "step {step}: waiters[{s}]");
                // Underflow canary: no counter may ever read as wrapped.
                assert!(m.depth < 1 << 30, "step {step}: depth wrapped");
                assert!(m.waiters < 1 << 30, "step {step}: waiters wrapped");
                assert!(m.hot_total <= m.take_total, "step {step}: hot > take");
            }
        }

        pub(super) async fn run(ops: Vec<Op>) {
            let store = TupleStore::transient(u32::MAX);
            let mut model = Model::default();
            for (step, op) in ops.iter().enumerate() {
                match *op {
                    Op::Put(s) => {
                        let id = store.put(&stage_name(s), b("p")).unwrap();
                        // A put pops (and skips) every dead waiter queued
                        // ahead of it on that stage.
                        model.dead_waiters.insert(s, 0);
                        model.queues.entry(s).or_default().push_back(id);
                    }
                    Op::TakeReady(s) => {
                        // Precondition: stage non-empty (else skip the op).
                        let Some(q) = model.queues.get_mut(&s) else { continue };
                        let Some(want) = q.pop_front() else { continue };
                        let (id, _) = store
                            .take(&stage_name(s), Duration::from_millis(100))
                            .await
                            .unwrap();
                        assert_eq!(id, want, "step {step}: FIFO violated");
                        model.inflight.push((id, s));
                    }
                    Op::TakeEmptyTimeout(s) => {
                        if model.queues.get(&s).is_some_and(|q| !q.is_empty()) {
                            continue; // only meaningful on an empty stage
                        }
                        let r = store
                            .take(&stage_name(s), Duration::from_millis(1))
                            .await;
                        assert!(matches!(r, Err(TupleError::Timeout)));
                        *model.dead_waiters.entry(s).or_default() += 1;
                        // Ensure the stage exists in metrics even if never put to.
                        model.queues.entry(s).or_default();
                    }
                    Op::AckOldest => {
                        if model.inflight.is_empty() {
                            continue;
                        }
                        let (id, _) = model.inflight.remove(0);
                        store.ack(id).unwrap();
                    }
                    Op::CompleteOldest(to) => {
                        if model.inflight.is_empty() {
                            continue;
                        }
                        let (id, _) = model.inflight.remove(0);
                        let new_id =
                            store.complete(id, &stage_name(to), b("c")).unwrap();
                        model.dead_waiters.insert(to, 0); // dispatch reaps them
                        model.queues.entry(to).or_default().push_back(new_id);
                    }
                    Op::RequeueAll => {
                        // Model stays exact only when re-queue order is
                        // unambiguous: restrict to ≤ 1 in-flight item.
                        if model.inflight.len() > 1 {
                            continue;
                        }
                        let requeued = store.requeue_expired(Duration::ZERO);
                        assert_eq!(requeued.len(), model.inflight.len());
                        if let Some((id, s)) = model.inflight.pop() {
                            assert_eq!(requeued, vec![id]);
                            // Re-queue dispatches: dead waiters on the target
                            // stage are popped and skipped, like any put.
                            model.dead_waiters.insert(s, 0);
                            model.queues.entry(s).or_default().push_back(id);
                        }
                    }
                }
                check(&store, &model, step);
            }
            // Drain to quiescence: every queued and in-flight item out, then
            // exact zeros everywhere.
            let stages: Vec<u8> = model.queues.keys().copied().collect();
            for s in stages {
                while model.queues.get(&s).is_some_and(|q| !q.is_empty()) {
                    let want = model.queues.get_mut(&s).unwrap().pop_front().unwrap();
                    let (id, _) = store
                        .take(&stage_name(s), Duration::from_millis(100))
                        .await
                        .unwrap();
                    assert_eq!(id, want);
                    store.ack(id).unwrap();
                }
            }
            for (id, _) in model.inflight.drain(..) {
                store.ack(id).unwrap();
            }
            assert_eq!(store.inflight_count(), 0);
            for m in store.metrics_snapshot() {
                assert_eq!(m.depth, 0, "quiescence: depth[{}]", m.stage);
                assert_eq!(m.inflight, 0, "quiescence: inflight[{}]", m.stage);
                // Dead waiters not yet reaped by a put are the one permitted
                // nonzero — but they must never read as wrapped.
                assert!(m.waiters < 1 << 30, "quiescence: waiters wrapped");
            }
        }

        proptest! {
            #![proptest_config(ProptestConfig {
                cases: 64, ..ProptestConfig::default()
            })]
            #[test]
            fn counters_match_reference_model(
                ops in proptest::collection::vec(op_strategy(), 1..120)
            ) {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                    .unwrap();
                rt.block_on(run(ops));
            }
        }
    }

    /// Run-41 falsification probe (semantic correctness): `state_chunk` pagination must
    /// neither lose nor duplicate live items across chunks, must drop items acked
    /// mid-scan, and must report done on an exhausted cursor.
    #[tokio::test]
    async fn state_chunk_paginates_without_loss_or_duplication() {
        let store = TupleStore::transient(500);
        let mut ids = Vec::new();
        for i in 0..10u32 {
            ids.push(store.put("s", b(&format!("x{i}"))).unwrap());
        }
        // Paginate with max_entries=3: collect all ids across chunks.
        let mut got = Vec::new();
        let mut cursor = 0u64;
        for _ in 0..10 {
            let (raw, next, done) = store.state_chunk(cursor, 3, usize::MAX);
            let records = decode_records(&raw);
            for r in &records {
                if let Record::Put { id, .. } = r {
                    got.push(*id);
                }
            }
            cursor = next;
            if done {
                break;
            }
        }
        got.sort_unstable();
        assert_eq!(got, ids, "pagination lost or duplicated live items");

        // Items acked mid-scan drop out of later chunks (at-least-once, never resurrection
        // *within* one scan): take+ack two items, rescan from 0.
        let (a, _) = store.take("s", Duration::from_millis(200)).await.unwrap();
        let (b2, _) = store.take("s", Duration::from_millis(200)).await.unwrap();
        store.ack(a).unwrap();
        store.ack(b2).unwrap();
        let (raw, _, done) = store.state_chunk(0, usize::MAX, usize::MAX);
        let live: Vec<u64> = decode_records(&raw)
            .iter()
            .filter_map(|r| match r {
                Record::Put { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        assert!(done);
        assert_eq!(live.len(), 8, "acked items must drop out of the state chunk");
        assert!(!live.contains(&a) && !live.contains(&b2), "acked ids resurrected");

        // Exhausted cursor → no item + done; the last chunk ends with the id fence (ER6-T).
        let (raw, _, done) = store.state_chunk(u64::MAX, 10, usize::MAX);
        let recs = decode_records(&raw);
        assert!(done && matches!(recs.as_slice(), [Record::Take { .. }]), "{recs:?}");
    }

    /// Run-42 falsification probe (semantic correctness): the documented lane invariant —
    /// "a keyed `take_by_key` and an unkeyed `take` on the same stage never interfere".
    /// An unkeyed take must never claim a keyed item (even when the FIFO is empty), and a
    /// keyed take claims exactly its key.
    #[tokio::test]
    async fn keyed_and_unkeyed_takes_never_interfere() {
        let store = TupleStore::transient(500);
        let keyed_id = store.put_keyed("s", Arc::from("k1"), b("keyed")).unwrap();
        let fifo_id = store.put("s", b("fifo")).unwrap();

        // Unkeyed take claims the FIFO item, never the keyed one.
        let (got, _) = store.take("s", Duration::from_millis(200)).await.unwrap();
        assert_eq!(got, fifo_id, "unkeyed take must claim the FIFO item");

        // FIFO now empty: an unkeyed take must TIME OUT rather than steal the keyed item.
        assert!(
            store.take("s", Duration::from_millis(200)).await.is_err(),
            "unkeyed take stole a keyed item"
        );

        // The keyed take claims exactly its item.
        let (got, _) = store.take_by_key("s", "k1", Duration::from_millis(200)).await.unwrap();
        assert_eq!(got, keyed_id);
    }
}
