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
    /// Run `f` — one re-assert, synchronous and short — unless the gate is closed. Returns whether it ran.
    pub fn write(&self, f: impl FnOnce()) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        self.in_flight.fetch_add(1, SeqCst);
        let open = !self.closed.load(SeqCst);
        if open { f(); }
        self.in_flight.fetch_sub(1, SeqCst);
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

/// Shared persist-loop primitive: ticks at `interval` and writes `payload_fn()`
/// to `kv_key` (Layer I) plus gossips it. Optional `on_tick` runs synchronously
/// before the KV write — used by `MeshHandle::advertise_persistent` to emit a
/// matching signal, and by capability ops with `None` (write only).
///
/// Tombstones `kv_key` at exit (cancel, shutdown, or sender drop), awaiting
/// channel capacity so the retraction is never silently dropped.
#[allow(clippy::too_many_arguments)]
pub async fn run_kv_persist_task(
    ctx:             Arc<CoreCtx>,
    mut cancel_rx:   tokio::sync::oneshot::Receiver<()>,
    mut shutdown_rx: watch::Receiver<bool>,
    kv_key:          Arc<str>,
    interval:        Duration,
    payload_fn:      PersistPayloadFn,
    on_tick:         Option<PersistOnTickFn>,
) {
    // Through the timer seam (item 6), one stream per persisted key.
    let mut ticker = crate::sim_seam::interval_ms(
        format!("kv-persist/{kv_key}"),
        interval.as_millis() as u64,
        tokio::time::MissedTickBehavior::Skip,
    );
    let mut first_tick = true;
    loop {
        tokio::select! { biased;
            _ = &mut cancel_rx               => break,
            _ = shutdown_rx.wait_for(|v| *v) => break,
            _ = ticker.tick() => {
                // Through the soft-state gate: once shutdown has closed it to sweep, a tick writes
                // nothing, so the sweep's tombstone stays the newest write (#627's review).
                ctx.soft_state_gate.write(|| {
                    let payload = payload_fn();
                    if let Some(ref f) = on_tick {
                        f(&ctx, &payload);
                    }
                    let update = crate::framing::make_gossip_update(
                        &ctx.node_id, ctx.default_ttl, Arc::clone(&kv_key), payload, false, &ctx.hlc,
                    );
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
    let tombstone = crate::framing::make_gossip_update(
        &ctx.node_id, ctx.default_ttl, Arc::clone(&kv_key), Bytes::new(), true, &ctx.hlc,
    );
    // Sign BEFORE the local apply (#611: the co-op provisioning demo). This tombstone runs as the
    // handle drops, often while the agent is shutting down; `shutdown_with_timeout` re-tombstones only
    // owned keys still live in the store, so once this apply lands the key is ours to deliver. Signing
    // after the apply put the (slow, in a debug build) signature between the apply and the send, the
    // shutdown signal landed in that gap, and the tombstone never left — peers kept a dead provider's
    // capability until its TTL. Signing first keeps apply → send as tight as the unsigned frame was.
    let msg = crate::framing::local_kv_wire_msg(&ctx, tombstone.clone());
    apply_and_notify(&ctx.kv_state, &tombstone);
    dispatch_gossip_send(&ctx.gossip_txs, msg, ctx.node_id.id_hash(), ForwardHint::All).await;
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
}
