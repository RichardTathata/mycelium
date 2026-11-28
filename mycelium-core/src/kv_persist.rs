//! [`run_kv_persist_task`] — the substrate's generic soft-state advertisement loop.
//!
//! A standing background task that re-asserts a single KV key every `interval`
//! tick (capability beacons, locality, `advertise_persistent`, …) and tombstones
//! it on exit (cancel, shutdown, or sender drop). Pure Layer I: it writes to the
//! store and gossips, nothing more. The first tick flips
//! [`CoreCtx::soft_state_advertised`](crate::CoreCtx) so the gateway readiness
//! probe can distinguish "process up" from "soft state hydrated".

use crate::context::CoreCtx;
use crate::framing::{dispatch_gossip_send, dispatch_gossip_try_send, ForwardHint};
use crate::store::apply_and_notify;
use bytes::Bytes;
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

/// The gate every soft-state re-assert passes through, closed by shutdown before its sweep.
///
/// Shutdown tombstones the node's own soft state (`cap/`, `req/`, `svc/`, `tools/`, `gcap/`,
/// `sys/load/{self}/…`) and then flushes the outbound queues before it signals the tasks to stop. A
/// task still ticking between the sweep and the signal wrote its key live again under a newer HLC, and
/// the flush delivered that instead (#627's review: 20/20 shutdowns at a 5 ms interval). A writer
/// passes [`write`](Self::write); shutdown calls [`close`](Self::close) before it sweeps, and from then
/// on no re-assert is written, while one already inside `write` finishes before `close` returns — so
/// the sweep's tombstone is the newest write of every key it covers.
///
/// Lock-free, so it adds no row to the lock-order table: a flag and an in-flight count, both
/// `SeqCst`. A writer increments the count and then reads the flag; `close` sets the flag and then
/// reads the count. In the single total order of `SeqCst` operations one of the two sees the other —
/// the writer backs out, or `close` waits for it. Weaker orderings do not give that guarantee.
#[derive(Debug, Default)]
pub struct SoftStateGate {
    closed:    std::sync::atomic::AtomicBool,
    in_flight: std::sync::atomic::AtomicUsize,
}

impl SoftStateGate {
    /// Run `f` — one re-assert — unless the gate is closed. Returns whether it ran.
    ///
    /// `close` waits for `f`, so `f` must be short and must not block or await: put only the apply and
    /// the send inside, and compute the payload (caller code, which may be slow) before. A panic in `f`
    /// is safe — the in-flight count is restored on unwind (#628's review: without that a later
    /// `close`, and so `shutdown`, spun forever).
    pub fn write(&self, f: impl FnOnce()) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        struct InFlight<'a>(&'a std::sync::atomic::AtomicUsize);
        impl Drop for InFlight<'_> {
            fn drop(&mut self) { self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst); }
        }
        self.in_flight.fetch_add(1, SeqCst);
        let _in_flight = InFlight(&self.in_flight);
        let open = !self.closed.load(SeqCst);
        if open { f(); }
        open
    }

    /// Close the gate and wait for any write already inside [`write`](Self::write) to finish. Idempotent.
    /// The wait is for synchronous code on another thread (a `write` holds no await), so it spins.
    pub fn close(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        self.closed.store(true, SeqCst);
        while self.in_flight.load(SeqCst) != 0 {
            std::thread::yield_now();
        }
    }

    /// Whether [`close`](Self::close) has been called.
    pub fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Closure that produces the payload bytes for one tick of [`run_kv_persist_task`].
pub type PersistPayloadFn = Arc<dyn Fn() -> Bytes + Send + Sync>;
/// Optional per-tick side-effect (e.g. a matching signal emission) invoked
/// synchronously before the KV write.
pub type PersistOnTickFn = Arc<dyn Fn(&Arc<CoreCtx>, &Bytes) + Send + Sync>;

// ── Registrations of one name on this node (ER10) ────────────────────────────────────────────────

/// **The live registrations of each key this node writes for a handle**, newest last — so a handle withdraws only
/// the promise it made (engineering review ER10, decision D3).
///
/// Several handles on one node can register one name — two `declare_requirement`s of one model, a replacement
/// prompt skill or MCP tool registered before the old one is dropped — and every one of them writes the same key
/// (`req/{node}/{ns}/{name}`, `cap/…`, `tools/{name}/{node}`). Until ER10 each handle's task tombstoned the key when
/// it ended, whoever else still held it, so the first to go withdrew the others' advertisement (R6-06, R10-03,
/// R22). Here the **newest** registration of a key is the one whose value the key carries; an older one stands by.
/// When a registration is withdrawn: if a newer one holds the key, nothing is written; if it held the key and an
/// older one remains, that one's value is written at once; only the last tombstones.
///
/// Node-local and above Layer I: nothing here changes what `apply_and_notify` accepts, and a peer's or another
/// member's write to the key is not looked at (detection, not prevention). The lock is a leaf (lock-order row 83):
/// each hold is a `Vec` scan and at most one push or remove, plus the caller's `then`, which must be short and
/// synchronous and take no lock — in this crate it only stamps an update (the HLC is an atomic), so **the order of
/// the stamps is the order of the decisions**, and last-writer-wins at every reader agrees with this table.
#[derive(Debug)]
pub struct Registrations<V> {
    inner: std::sync::Mutex<RegistrationsInner<V>>,
}

#[derive(Debug)]
struct RegistrationsInner<V> {
    next: u64,
    held: std::collections::HashMap<Arc<str>, Vec<(u64, V)>>,
}

impl<V> Default for Registrations<V> {
    fn default() -> Self {
        Self { inner: std::sync::Mutex::new(RegistrationsInner { next: 1, held: std::collections::HashMap::new() }) }
    }
}

/// What a withdrawn registration finds, handed to [`Registrations::withdraw`]'s `then` under the lock.
#[derive(Debug)]
pub enum Withdrawal<'a, V> {
    /// A newer registration holds the key, or this one was withdrawn already: the record is not this one's.
    NotHolder,
    /// This one held the key; the newest that remains (its value given) holds it now.
    Successor(&'a V),
    /// This was the last registration of the key: it is no one's now.
    Last,
}

impl<V> Registrations<V> {
    fn lock(&self) -> std::sync::MutexGuard<'_, RegistrationsInner<V>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Register `value` under `key` as its newest registration, and run `then` on it under the lock. Returns the
    /// registration's id, which [`update`](Self::update), [`holds`](Self::holds) and [`withdraw`](Self::withdraw)
    /// take, with what `then` returned.
    pub fn register<R>(&self, key: &Arc<str>, value: V, then: impl FnOnce(&V) -> R) -> (u64, R) {
        let mut inner = self.lock();
        let id = inner.next;
        inner.next += 1;
        let list = inner.held.entry(Arc::clone(key)).or_default();
        list.push((id, value));
        let r = then(&list[list.len() - 1].1);
        (id, r)
    }

    /// Replace registration `id`'s value, and run `then` on it under the lock **only if it holds the key** (it is
    /// the newest). `None` when it does not, or is not registered.
    pub fn update<R>(&self, key: &str, id: u64, value: V, then: impl FnOnce(&V) -> R) -> Option<R> {
        let mut inner = self.lock();
        let list = inner.held.get_mut(key)?;
        let pos = list.iter().position(|(i, _)| *i == id)?;
        let old = std::mem::replace(&mut list[pos].1, value);
        let r = (pos + 1 == list.len()).then(|| then(&list[pos].1));
        drop(inner);
        drop(old); // A value's own `Drop` runs outside the lock.
        r
    }

    /// Whether registration `id` holds `key` — it is the newest registered. A snapshot: another registration may
    /// be made, or this one withdrawn, as soon as it returns.
    pub fn holds(&self, key: &str, id: u64) -> bool {
        self.lock().held.get(key).and_then(|l| l.last()).is_some_and(|(i, _)| *i == id)
    }

    /// How many registrations of `key` are live.
    pub fn count(&self, key: &str) -> usize {
        self.lock().held.get(key).map_or(0, Vec::len)
    }

    /// Withdraw registration `id` of `key` and run `then`, under the lock, on what that leaves (see
    /// [`Withdrawal`]). Withdrawing an id twice, or one never registered, is [`Withdrawal::NotHolder`].
    pub fn withdraw<R>(&self, key: &str, id: u64, then: impl FnOnce(Withdrawal<'_, V>) -> R) -> R {
        let mut inner = self.lock();
        let Some(list) = inner.held.get_mut(key) else { return then(Withdrawal::NotHolder) };
        let Some(pos) = list.iter().position(|(i, _)| *i == id) else { return then(Withdrawal::NotHolder) };
        let held = pos + 1 == list.len();
        // Kept until the lock is released: the withdrawn value's own `Drop` (the last `Arc` of a prompt skill's
        // backend, say) must not run under the table's lock (review round 1).
        let removed = list.remove(pos);
        let r = if !held {
            then(Withdrawal::NotHolder)
        } else if list.is_empty() {
            inner.held.remove(key);
            then(Withdrawal::Last)
        } else {
            then(Withdrawal::Successor(&list[list.len() - 1].1))
        };
        drop(inner);
        drop(removed);
        r
    }
}

/// **One handle's registration of a soft-state key on this node** ([`Registrations`], ER10): the key carries the
/// newest registration's value, and withdrawing this one writes only what that leaves — nothing while a newer one
/// holds the key, the older one's value when this one held it, a tombstone when it was the last.
///
/// Withdrawn by [`withdraw`](Self::withdraw), which awaits gossip capacity for a tombstone; dropped without it (a
/// task aborted, a future never polled), it withdraws on drop and queues what it writes with a try-send, so a
/// registration never outlives its handle and never holds a key nobody serves.
pub struct HeldKey {
    ctx:       Arc<CoreCtx>,
    key:       Arc<str>,
    id:        u64,
    withdrawn: bool,
}

impl HeldKey {
    /// Register `key` with no value yet — a refresher's, which writes on its first tick ([`run_kv_persist_task`]).
    /// As the newest registration it holds the key at once: an older one stops refreshing it.
    pub fn register(ctx: &Arc<CoreCtx>, key: Arc<str>) -> Self {
        let (id, ()) = ctx.soft_state_holders.register(&key, None, |_| ());
        Self { ctx: Arc::clone(ctx), key, id, withdrawn: false }
    }

    /// Register `value` under `key` and write it now — a key written once, as an MCP tool's schema is: applied,
    /// handed to the WAL (try-send) and gossiped (try-send), as [`ops::kv_set`](crate::ops::kv_set) writes. A value
    /// too large for one gossip frame is refused as `kv_set` refuses it — logged, nothing written — and **nothing is
    /// registered** either (ER10 review round 1): the returned registration never holds the key and withdraws
    /// nothing, so an older registration keeps the key, its value and the name. Registering it anyway made the
    /// newest registration one whose value no key carried, and its loop the only one serving the name.
    pub fn register_written(ctx: &Arc<CoreCtx>, key: Arc<str>, value: Bytes) -> Self {
        if key.len() + value.len() > crate::framing::MAX_KV_WRITE_BYTES {
            tracing::warn!(key = %key, size = key.len() + value.len(), limit = crate::framing::MAX_KV_WRITE_BYTES,
                "kv write rejected: key + value cannot fit a gossip frame; nothing is registered for this name");
            return Self { ctx: Arc::clone(ctx), key, id: 0, withdrawn: true };
        }
        let (id, update) = ctx.soft_state_holders.register(&key, Some(value), |v| {
            let v = v.clone().unwrap_or_default();
            crate::framing::make_gossip_update(&ctx.node_id, ctx.default_ttl, Arc::clone(&key), v, false, &ctx.hlc)
        });
        apply_and_notify(&ctx.kv_state, &update);
        if let Some(wal) = ctx.wal.get() {
            wal.append_try(crate::framing::sync_entry_from(&update));
        }
        dispatch_gossip_try_send(
            &ctx.gossip_txs, crate::framing::local_kv_wire_msg(ctx, update),
            ctx.node_id.id_hash(), ForwardHint::All, &ctx.kv_state.dropped_frames,
        );
        Self { ctx: Arc::clone(ctx), key, id, withdrawn: false }
    }

    /// The key this registration is of.
    pub fn key(&self) -> &Arc<str> {
        &self.key
    }

    /// Whether this registration holds its key — it is the newest — so its value is the one the key carries, and
    /// the one a caller of the name is served by. A snapshot (see [`Registrations::holds`]).
    pub fn holds(&self) -> bool {
        // Ids start at 1; 0 is a registration refused at `register_written`, which never holds its key.
        self.id != 0 && self.ctx.soft_state_holders.holds(&self.key, self.id)
    }

    /// Record `value` as this registration's newest, and, if it holds the key, the update that writes it (stamped
    /// under the table's lock). A refresher's tick.
    fn refresh(&self, value: Bytes) -> Option<crate::framing::GossipUpdate> {
        let ctx = &self.ctx;
        ctx.soft_state_holders.update(&self.key, self.id, Some(value), |v| {
            let v = v.clone().unwrap_or_default();
            crate::framing::make_gossip_update(&ctx.node_id, ctx.default_ttl, Arc::clone(&self.key), v, false, &ctx.hlc)
        })
    }

    /// Withdraw from the table, once, and stamp what that leaves to write: nothing, the successor's value, or a
    /// tombstone (also when the successor has written no value yet — its own first write follows, newer).
    fn take_withdrawal(&mut self) -> Option<crate::framing::GossipUpdate> {
        if std::mem::replace(&mut self.withdrawn, true) {
            return None;
        }
        let ctx = &self.ctx;
        let key = &self.key;
        ctx.soft_state_holders.withdraw(key, self.id, |w| {
            let (value, tombstone) = match w {
                Withdrawal::NotHolder => return None,
                Withdrawal::Successor(Some(v)) => (v.clone(), false),
                Withdrawal::Successor(None) | Withdrawal::Last => (Bytes::new(), true),
            };
            Some(crate::framing::make_gossip_update(&ctx.node_id, ctx.default_ttl, Arc::clone(key), value, tombstone, &ctx.hlc))
        })
    }

    /// Withdraw this registration and write what that leaves, awaiting gossip capacity for it.
    ///
    /// A tombstone is signed before its local apply (#611: this often runs as the agent shuts down; shutdown
    /// re-tombstones only owned keys still live, so once the apply lands the key is ours to deliver, and the slow
    /// signature must not sit between the apply and the send). A successor's value is a re-assert and passes the
    /// soft-state gate: after shutdown has closed it to sweep, nothing live is written.
    pub async fn withdraw(mut self) {
        let Some(update) = self.take_withdrawal() else { return };
        let ctx = Arc::clone(&self.ctx);
        let msg = crate::framing::local_kv_wire_msg(&ctx, update.clone());
        if update.is_tombstone {
            apply_and_notify(&ctx.kv_state, &update);
        } else if !ctx.soft_state_gate.write(|| apply_and_notify(&ctx.kv_state, &update)) {
            return;
        }
        dispatch_gossip_send(&ctx.gossip_txs, msg, ctx.node_id.id_hash(), ForwardHint::All).await;
    }
}

impl std::fmt::Debug for HeldKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeldKey").field("key", &self.key).field("id", &self.id).field("withdrawn", &self.withdrawn).finish()
    }
}

impl Drop for HeldKey {
    fn drop(&mut self) {
        let Some(update) = self.take_withdrawal() else { return };
        let ctx = &self.ctx;
        let msg = crate::framing::local_kv_wire_msg(ctx, update.clone());
        if update.is_tombstone {
            apply_and_notify(&ctx.kv_state, &update);
        } else if !ctx.soft_state_gate.write(|| apply_and_notify(&ctx.kv_state, &update)) {
            return;
        }
        dispatch_gossip_try_send(&ctx.gossip_txs, msg, ctx.node_id.id_hash(), ForwardHint::All, &ctx.kv_state.dropped_frames);
    }
}

/// Shared persist-loop primitive: ticks at `interval` and writes `payload_fn()`
/// to `kv_key` (Layer I) plus gossips it. Optional `on_tick` runs synchronously
/// before the KV write — used by `MeshHandle::advertise_persistent` to emit a
/// matching signal, and by capability ops with `None` (write only).
///
/// **One registration of `kv_key`** ([`HeldKey`], ER10), made when this is called — not when the task is first
/// polled — so registrations are ordered as their handles were made. While a newer registration of the key on this
/// node is live, this one computes its payload but writes nothing (and runs no `on_tick`); at exit it writes only
/// what its withdrawal leaves: nothing under a newer registration, the older one's payload when this one held the
/// key, a tombstone when it was the last — awaiting channel capacity so the retraction is never silently dropped.
#[allow(clippy::too_many_arguments)]
pub fn run_kv_persist_task(
    ctx:         Arc<CoreCtx>,
    cancel_rx:   tokio::sync::oneshot::Receiver<()>,
    shutdown_rx: watch::Receiver<bool>,
    kv_key:      Arc<str>,
    interval:    Duration,
    payload_fn:  PersistPayloadFn,
    on_tick:     Option<PersistOnTickFn>,
) -> impl std::future::Future<Output = ()> + Send + 'static {
    let held = HeldKey::register(&ctx, kv_key);
    persist_loop(ctx, cancel_rx, shutdown_rx, held, interval, payload_fn, on_tick)
}

async fn persist_loop(
    ctx:             Arc<CoreCtx>,
    mut cancel_rx:   tokio::sync::oneshot::Receiver<()>,
    mut shutdown_rx: watch::Receiver<bool>,
    held:            HeldKey,
    interval:        Duration,
    payload_fn:      PersistPayloadFn,
    on_tick:         Option<PersistOnTickFn>,
) {
    // Through the timer seam (item 6), one stream per persisted key.
    let mut ticker = crate::sim_seam::interval_ms(
        format!("kv-persist/{}", held.key()),
        interval.as_millis() as u64,
        tokio::time::MissedTickBehavior::Skip,
    );
    let mut first_tick = true;
    loop {
        tokio::select! { biased;
            _ = &mut cancel_rx               => break,
            _ = shutdown_rx.wait_for(|v| *v) => break,
            _ = ticker.tick() => {
                // The caller's closures run outside the soft-state gate (they may be slow, or panic);
                // only the stamp, the apply and the send go through it. Once shutdown has closed it to sweep, a
                // tick writes nothing, so the sweep's tombstone stays the newest write (#627's review).
                if ctx.soft_state_gate.is_closed() { continue; }
                let payload = payload_fn();
                // A standby (a newer registration holds the key) emits nothing either: the signal would announce
                // a value the key does not carry. A snapshot — at a replacement's edge one extra signal may go.
                if let Some(ref f) = on_tick
                    && held.holds()
                {
                    f(&ctx, &payload);
                }
                ctx.soft_state_gate.write(|| {
                    let Some(update) = held.refresh(payload) else { return };
                    apply_and_notify(&ctx.kv_state, &update);
                    if first_tick {
                        ctx.soft_state_advertised.store(true, std::sync::atomic::Ordering::Release);
                        first_tick = false;
                    }
                    dispatch_gossip_try_send(
                        &ctx.gossip_txs, crate::framing::local_kv_wire_msg(&ctx, update),
                        ctx.node_id.id_hash(), ForwardHint::All, &ctx.kv_state.dropped_frames,
                    );
                });
            }
        }
    }
    held.withdraw().await;
}

#[cfg(test)]
mod gate_tests {
    use super::SoftStateGate;
    use std::sync::{atomic::{AtomicBool, Ordering}, Arc, Barrier};

    /// A closed gate writes nothing, and closing is idempotent.
    #[test]
    fn a_closed_gate_runs_nothing() {
        let gate = SoftStateGate::default();
        assert!(gate.write(|| {}), "an open gate runs the write");
        gate.close();
        gate.close();
        let ran = AtomicBool::new(false);
        assert!(!gate.write(|| ran.store(true, Ordering::SeqCst)), "a closed gate reports it did not run");
        assert!(!ran.load(Ordering::SeqCst) && gate.is_closed());
    }

    /// `close` returns only after a write already inside the gate has finished — the property the
    /// shutdown sweep relies on, so the sweep's tombstone is the newest write.
    #[test]
    fn close_waits_for_a_write_in_flight() {
        let gate = Arc::new(SoftStateGate::default());
        let (entered, finished) = (Arc::new(Barrier::new(2)), Arc::new(AtomicBool::new(false)));
        let writer = {
            let (gate, entered, finished) = (Arc::clone(&gate), Arc::clone(&entered), Arc::clone(&finished));
            std::thread::spawn(move || gate.write(|| {
                entered.wait();
                std::thread::sleep(std::time::Duration::from_millis(50));
                finished.store(true, Ordering::SeqCst);
            }))
        };
        entered.wait();
        gate.close();
        assert!(finished.load(Ordering::SeqCst), "close returned while a write was still inside the gate");
        assert!(writer.join().unwrap(), "the write that was already inside ran");
    }

    /// A write that panics leaves the gate closable (#628's review, finding 1): the in-flight count is
    /// restored on unwind, so a later `close` returns instead of spinning forever. `close` runs on its own
    /// thread with a bound, so the unfixed code fails here rather than hanging the suite.
    #[test]
    fn a_write_that_panics_does_not_wedge_close() {
        let gate = Arc::new(SoftStateGate::default());
        let g = Arc::clone(&gate);
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| { g.write(|| panic!("a write panicked")); }));
        assert!(unwound.is_err(), "the panic propagated to the writer");
        let (tx, rx) = std::sync::mpsc::channel();
        let g = Arc::clone(&gate);
        std::thread::spawn(move || { g.close(); let _ = tx.send(()); });
        assert!(rx.recv_timeout(std::time::Duration::from_secs(2)).is_ok(),
            "close() did not return after a write panicked inside the gate");
    }
}

#[cfg(test)]
mod registrations_tests {
    use super::{Registrations, Withdrawal};
    use std::sync::Arc;

    fn outcome(w: Withdrawal<'_, &'static str>) -> String {
        match w {
            Withdrawal::NotHolder => "not-holder".into(),
            Withdrawal::Successor(v) => format!("successor:{v}"),
            Withdrawal::Last => "last".into(),
        }
    }

    /// The newest registration holds the key; withdrawing an older one leaves it; withdrawing the holder hands the
    /// key to the newest that remains; only the last is `Last`; a second withdrawal of one id is `NotHolder`.
    #[test]
    fn the_newest_holds_and_only_the_last_withdraws_the_key() {
        let r = Registrations::<&'static str>::default();
        let k: Arc<str> = Arc::from("req/n/llm/m");
        let (a, ()) = r.register(&k, "a", |_| ());
        let (b, ()) = r.register(&k, "b", |_| ());
        let (c, ()) = r.register(&k, "c", |_| ());
        assert!(r.holds(&k, c) && !r.holds(&k, a) && !r.holds(&k, b));
        assert_eq!(r.update(&k, a, "a2", |v| *v), None, "a standby's update runs nothing");
        assert_eq!(r.update(&k, c, "c2", |v| *v), Some("c2"), "the holder's update runs");
        assert_eq!(r.withdraw(&k, b, outcome), "not-holder", "a middle registration leaves the key");
        assert_eq!(r.withdraw(&k, c, outcome), "successor:a2", "the holder hands the key to the newest that remains");
        assert_eq!(r.withdraw(&k, c, outcome), "not-holder", "withdrawn twice");
        assert!(r.holds(&k, a));
        assert_eq!(r.withdraw(&k, a, outcome), "last");
        assert_eq!(r.count(&k), 0);
        assert_eq!(r.withdraw(&k, a, outcome), "not-holder", "nothing left");
    }

    /// Keys are independent: one name's registrations never touch another's.
    #[test]
    fn keys_are_independent() {
        let r = Registrations::<u8>::default();
        let (k1, k2): (Arc<str>, Arc<str>) = (Arc::from("tools/a/n"), Arc::from("tools/b/n"));
        let (a, ()) = r.register(&k1, 1, |_| ());
        let (b, ()) = r.register(&k2, 2, |_| ());
        assert!(r.holds(&k1, a) && r.holds(&k2, b) && !r.holds(&k1, b));
        assert!(r.withdraw(&k1, a, |w| matches!(w, Withdrawal::Last)));
        assert!(r.holds(&k2, b));
    }
}
