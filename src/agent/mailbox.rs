//! Actor/Event mailboxes — KV-backed durable event delivery.
//!
//! [`ServiceHandle::deliver_event`] writes an event into the mesh at a
//! HLC-ordered key under `mailbox/{target}/{kind}/{hlc_hex}/{sender}`. Any node
//! that knows the target's [`NodeId`] can deliver; the event propagates
//! via the normal gossip KV path. The target's [`open_mailbox`] watcher
//! picks up new entries in causal order, delivers them to a channel, and
//! tombstones each entry so it is not redelivered after restart.
//!
//! ## Delivery guarantees
//!
//! - **At-least-once** while the event is in the KV. Events stored in KV
//!   propagate to all live nodes via anti-entropy; a crashed target node
//!   receives its pending events on reconnect. An event has no expiry of its
//!   own: the `default_ttl` its write carries is the gossip hop count, not a
//!   lifetime, so an undelivered event stays until it is delivered and deleted.
//! - **Causal order** within the watcher task: entries are sorted by their
//!   HLC stamp, then key, before delivery, so causal happens-before is preserved;
//!   two senders' events at one stamp are both kept (the key names its sender)
//!   and come in key order.
//! - **Tombstone-on-delivery**: entries are deleted from the KV store
//!   immediately after delivery and the tombstone is gossiped, preventing
//!   redelivery on restart. *Delivery* differs by consumer: the in-process
//!   [`ServiceHandle::open_mailbox`](crate::ServiceHandle::open_mailbox) deletes an
//!   entry when it enters the receiver's channel (the library cannot see a plain
//!   `mpsc::Receiver` being read, so a receiver dropped with events queued loses
//!   them); the gateway's `GET /gateway/mailbox/{kind}` deletes one only when its
//!   stream hands the event to the HTTP connection ([`open_mailbox_acked`], post-360
//!   hardening row E), so a dropped stream loses at most what the connection had
//!   already accepted.
//!
//! ## KV namespace
//!
//! `mailbox/{target_node_id}/{kind}/{hlc_ts:016x}/{sender_node_id}` → `sender_node_id_len(2 LE) | sender_bytes | payload`
//!
//! The sender segment (ER1) keeps two nodes' events apart when their clocks hand out one stamp; the 2.40
//! shape, `…/{hlc_ts:016x}` with no sender, is still read. An entry under a mailbox's prefix whose remainder
//! is neither shape belongs to another kind (one containing `/`) and is left alone. A receiver older than
//! this shape delivers a salted event with `hlc_ts == 0`: upgrade receivers before or with senders.

use crate::framing::{ForwardHint, dispatch_gossip_try_send, make_gossip_update};
use crate::node_id::NodeId;
use crate::store::{apply_and_notify, scan_kv_prefix};
use bytes::{BufMut, Bytes, BytesMut};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use super::TaskCtx;

/// An event received from a node's mailbox.
pub struct MeshEvent {
    pub kind:    Arc<str>,
    pub sender:  NodeId,
    pub payload: Bytes,
    /// Packed HLC timestamp from the delivery key — useful for causal ordering.
    pub hlc_ts:  u64,
}

/// A mailbox event whose entry is deleted only when [`MailboxAck::ack`] is called — the consumer says
/// it has handed the event on (post-360 hardening row E). Dropping it un-acked leaves the entry in the
/// store and releases it to every other stream of the same mailbox (and to the next one opened).
#[cfg_attr(not(feature = "gateway"), allow(dead_code))] // the gateway's mailbox stream is the one consumer
pub(crate) struct AckedEvent {
    pub(crate) event: MeshEvent,
    pub(crate) ack:   MailboxAck,
}

/// Deletes one delivered mailbox entry. See [`AckedEvent`].
#[cfg_attr(not(feature = "gateway"), allow(dead_code))] // the gateway's mailbox stream is the one consumer
pub(crate) struct MailboxAck {
    ctx:       Arc<TaskCtx>,
    key:       Arc<str>,
    in_flight: InFlight,
    acked:     bool,
}

/// The keys handed to a consumer of one mailbox `(node, kind)` and not yet acknowledged — **shared by
/// every acked stream of that mailbox** (#613's review, finding 2: a per-stream set let a second stream
/// deliver everything the first had handed out). A drain skips them, so an entry is never handed out
/// twice while its first delivery is outstanding; `released` ticks when one is let go un-acked, so the
/// other streams look again.
pub(crate) struct InFlightSet {
    keys:     papaya::HashMap<Arc<str>, ()>,
    released: tokio::sync::watch::Sender<u64>,
    /// The registry this set is listed in, and its key there: removed from it when the last holder drops.
    registry: MailboxInFlight,
    prefix:   Arc<str>,
}

type InFlight = Arc<InFlightSet>;

/// `TaskCtx::mailbox_in_flight`: one live [`InFlightSet`] per mailbox prefix, held weakly — an entry
/// lives while a stream of that mailbox does, and is removed when its set drops.
pub(crate) type MailboxInFlight = Arc<papaya::HashMap<Arc<str>, std::sync::Weak<InFlightSet>>>;

impl Drop for InFlightSet {
    fn drop(&mut self) {
        // Retry-safe: removes only a dead entry; a set a concurrent opener just listed is left alone.
        self.registry.pin().compute(Arc::clone(&self.prefix), |e| match e {
            Some((_, w)) if w.strong_count() == 0 => papaya::Operation::Remove,
            _ => papaya::Operation::Abort(()),
        });
    }
}

/// The shared in-flight set for `prefix`: the live one, or a new one listed in its place.
#[cfg_attr(not(feature = "gateway"), allow(dead_code))]
fn shared_in_flight(registry: &MailboxInFlight, prefix: &Arc<str>) -> InFlight {
    loop {
        if let Some(live) = registry.pin().get(prefix).and_then(std::sync::Weak::upgrade) {
            return live;
        }
        let fresh = Arc::new(InFlightSet {
            keys:     papaya::HashMap::new(),
            released: tokio::sync::watch::channel(0).0,
            registry: Arc::clone(registry),
            prefix:   Arc::clone(prefix),
        });
        // Retry-safe: the closure only reads the entry and downgrades `fresh`.
        let listed = !matches!(registry.pin().compute(Arc::clone(prefix), |e| match e {
            Some((_, w)) if w.strong_count() > 0 => papaya::Operation::Abort(()),
            _ => papaya::Operation::Insert(Arc::downgrade(&fresh)),
        }), papaya::Compute::Aborted(()));
        if listed {
            return fresh;
        }
        // Another opener listed a live set first: use it. `fresh` drops here; its `Drop` leaves the
        // live entry alone.
    }
}

#[cfg_attr(not(feature = "gateway"), allow(dead_code))] // the gateway's mailbox stream is the one consumer
impl MailboxAck {
    /// The consumer has the event: delete its entry. The tombstone lands **before** the key leaves
    /// the in-flight set, so a drain that finds the key absent finds it deleted (`drain_prefix`
    /// claims the key, then checks the store).
    pub(crate) fn ack(mut self) {
        tombstone(&self.ctx, Arc::clone(&self.key));
        self.in_flight.keys.pin().remove(&self.key);
        self.acked = true;
    }
}

impl Drop for MailboxAck {
    fn drop(&mut self) {
        if !self.acked {
            // Handed out and never acknowledged (the stream dropped with it queued): the entry stays in
            // the store, and every stream of this mailbox may hand it out again.
            self.in_flight.keys.pin().remove(&self.key);
            self.in_flight.released.send_modify(|n| *n = n.wrapping_add(1));
        }
    }
}

/// Where a drain hands its events.
enum Sink {
    /// The in-process mailbox: an entry is deleted as it enters the channel.
    Immediate(mpsc::Sender<MeshEvent>),
    /// The gateway's mailbox: an entry is deleted when the consumer acks it.
    #[cfg_attr(not(feature = "gateway"), allow(dead_code))]
    Acked { tx: mpsc::Sender<AckedEvent>, in_flight: InFlight },
}

/// Cancels the corresponding [`ServiceHandle::open_mailbox`]
/// background task on drop.
pub struct MailboxHandle {
    pub(crate) _cancel: oneshot::Sender<()>,
}

fn encode_value(sender: &NodeId, payload: &Bytes) -> Bytes {
    let s = sender.to_string();
    let sb = s.as_bytes();
    let mut buf = BytesMut::with_capacity(2 + sb.len() + payload.len());
    buf.put_u16_le(sb.len() as u16);
    buf.put(sb);
    buf.put(payload.as_ref());
    buf.freeze()
}

fn decode_value(bytes: &Bytes) -> Option<(NodeId, Bytes)> {
    if bytes.len() < 2 { return None; }
    let len = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
    if bytes.len() < 2 + len { return None; }
    let sender_str = std::str::from_utf8(&bytes[2..2 + len]).ok()?;
    let sender: NodeId = sender_str.parse().ok()?;
    let payload = bytes.slice(2 + len..);
    Some((sender, payload))
}

/// The prefix every entry of mailbox `(node, kind)` sits under: `mailbox/{node}/{kind}/`.
pub(crate) fn mailbox_prefix(node: &NodeId, kind: &str) -> String {
    format!("mailbox/{node}/{kind}/")
}

/// The key of one event: `mailbox/{target}/{kind}/{hlc:016x}/{sender}` — the log route's shape (ER1). One
/// node's HLC never hands out a stamp twice, so the sender segment keeps two nodes' events at one stamp
/// apart; it comes after the stamp, so key order within a mailbox is still stamp order.
pub(crate) fn mailbox_key(target: &NodeId, kind: &str, hlc: u64, sender: &NodeId) -> String {
    format!("mailbox/{target}/{kind}/{hlc:016x}/{sender}")
}

/// The stamp of an entry from its key's remainder after [`mailbox_prefix`]: `{hlc:016x}` (2.40 and earlier)
/// or `{hlc:016x}/{sender}` — exactly sixteen hex digits, then nothing or one non-empty segment with no
/// further `/`. `None` is neither shape: another kind's entry (`a/b`'s under mailbox `a`), not this
/// mailbox's to deliver or delete.
fn parse_suffix(suffix: &str) -> Option<u64> {
    let (hex, sender) = match suffix.split_once('/') {
        Some((hex, sender)) => (hex, Some(sender)),
        None => (suffix, None),
    };
    if hex.len() != 16 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) { return None; }
    if sender.is_some_and(|s| s.is_empty() || s.contains('/')) { return None; }
    u64::from_str_radix(hex, 16).ok()
}

fn tombstone(ctx: &Arc<TaskCtx>, key: Arc<str>) {
    let update = make_gossip_update(
        &ctx.node_id, ctx.default_ttl, key, Bytes::new(), true, &ctx.hlc,
    );
    apply_and_notify(&ctx.kv_state, &update);
    dispatch_gossip_try_send(
        &ctx.gossip_txs,
        crate::framing::local_kv_wire_msg(ctx, update),
        ctx.node_id.id_hash(),
        ForwardHint::All,
        &ctx.kv_state.dropped_frames,
    );
}

/// Maximum entries delivered (and tombstoned) per `drain_prefix` call.
///
/// Bounding the batch prevents the watcher task from blocking indefinitely
/// when a large backlog accumulates (e.g. after a long network partition).
/// After each chunk the watcher yields to the runtime; if more entries remain
/// the prefix watcher fires again immediately and the next chunk is processed.
const DRAIN_CHUNK: usize = 64;

/// Delivers up to `DRAIN_CHUNK` mailbox entries in `(hlc, key)` order, tombstoning
/// each on delivery (or, for an acked sink, when the consumer acks it). Yields
/// between chunks so the tokio runtime can run other tasks. Stops early if the
/// receiver has been dropped.
async fn drain_prefix(
    ctx:    &Arc<TaskCtx>,
    prefix: &Arc<str>,
    kind:   &Arc<str>,
    sink:   &Sink,
) {
    loop {
        // One pass keeps what this drain may send: an entry whose remainder is neither mailbox shape is
        // another kind's (`a/b`'s under mailbox `a`) and is left alone — not delivered, not deleted; one
        // handed out by any stream of this mailbox and not yet acked is not this drain's to send. Both leave
        // the scan **before** the emptiness check, or a foreign entry would keep the loop from ending.
        let mut entries: Vec<(u64, Arc<str>, Bytes)> = {
            let in_flight = match sink {
                Sink::Acked { in_flight, .. } => Some(in_flight.keys.pin()),
                Sink::Immediate(_) => None,
            };
            scan_kv_prefix(&ctx.kv_state, prefix.as_ref())
                .into_iter()
                .filter(|(key, _)| in_flight.as_ref().is_none_or(|p| !p.contains_key(key)))
                .filter_map(|(key, value)| {
                    let hlc = parse_suffix(key.strip_prefix(prefix.as_ref())?)?;
                    Some((hlc, key, value))
                })
                .collect()
        };
        if entries.is_empty() { break; }
        // By stamp, then key: two senders' events at one stamp have distinct keys (the sender segment),
        // and at one stamp an old-shape key (`…/{hlc}`) sorts before a salted one.
        entries.sort_unstable_by(|(ha, ka, _), (hb, kb, _)| (ha, ka).cmp(&(hb, kb)));

        let chunk: Vec<_> = entries.drain(..entries.len().min(DRAIN_CHUNK)).collect();
        for (hlc_ts, key, value) in chunk {
            let Some((sender, payload)) = decode_value(&value) else {
                tombstone(ctx, key);  // malformed — evict silently
                continue;
            };
            let event  = MeshEvent { kind: Arc::clone(kind), sender, payload, hlc_ts };
            match sink {
                Sink::Immediate(tx) => {
                    if tx.send(event).await.is_err() {
                        return; // receiver dropped
                    }
                    tombstone(ctx, key);
                }
                Sink::Acked { tx, in_flight } => {
                    // Claim the key first — atomically, so two streams of one mailbox never both hand
                    // it out — then check it is still live: the scan may predate an ack, and an ack
                    // tombstones before it releases the key, so a key this drain could claim and find
                    // live was not acked.
                    if in_flight.keys.pin().try_insert(Arc::clone(&key), ()).is_err() { continue; }
                    if ctx.kv_state.store.pin().get(key.as_ref()).is_none_or(|e| e.data.is_none()) {
                        in_flight.keys.pin().remove(&key);
                        continue;
                    }
                    let ack = MailboxAck { ctx: Arc::clone(ctx), key, in_flight: Arc::clone(in_flight), acked: false };
                    if tx.send(AckedEvent { event, ack }).await.is_err() {
                        return; // receiver dropped: the un-acked event's drop released the key
                    }
                }
            }
        }
        // Yield so the runtime can service other tasks between chunks.
        tokio::task::yield_now().await;
    }
}

async fn mailbox_task(
    ctx:         Arc<TaskCtx>,
    prefix:      Arc<str>,
    kind:        Arc<str>,
    sink:        Sink,
    mut cancel_rx:   oneshot::Receiver<()>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    mut watcher:     tokio::sync::watch::Receiver<u64>,
) {
    // An acked mailbox also looks again when another stream of it lets an event go un-acked. Subscribed
    // **before** the first drain, so a release during it is seen (#613's final review).
    let mut released = match &sink {
        Sink::Acked { in_flight, .. } => Some(in_flight.released.subscribe()),
        Sink::Immediate(_) => None,
    };
    // Deliver any entries already present before we started watching.
    drain_prefix(&ctx, &prefix, &kind, &sink).await;

    loop {
        tokio::select! { biased;
            _ = &mut cancel_rx => break,
            result = shutdown_rx.changed() => {
                if result.is_err() || *shutdown_rx.borrow() { break; }
            }
            _ = watcher.changed() => {
                drain_prefix(&ctx, &prefix, &kind, &sink).await;
            }
            Ok(()) = async { match released.as_mut() { Some(r) => r.changed().await, None => std::future::pending().await } } => {
                drain_prefix(&ctx, &prefix, &kind, &sink).await;
            }
        }
    }
}

/// Delivers an event to `target`'s mailbox on behalf of `self_node_id`.
///
/// Free-function variant of [`ServiceHandle::deliver_event`] for callers that
/// hold only `Arc<TaskCtx>` (e.g. the HTTP gateway handler).
pub(crate) fn deliver_event_ctx(
    ctx:          &Arc<TaskCtx>,
    self_node_id: &NodeId,
    target:       &NodeId,
    kind:         Arc<str>,
    payload:      Bytes,
) -> bool {
    deliver_event_at(ctx, self_node_id, target, kind, payload, ctx.hlc.tick())
}

/// The body of [`deliver_event_ctx`], at a stamp the caller chose — the test seam.
pub(crate) fn deliver_event_at(
    ctx:          &Arc<TaskCtx>,
    self_node_id: &NodeId,
    target:       &NodeId,
    kind:         Arc<str>,
    payload:      Bytes,
    ts:           u64,
) -> bool {
    let key: Arc<str> = Arc::from(mailbox_key(target, &kind, ts, self_node_id).as_str());
    let value = encode_value(self_node_id, &payload);
    let update = make_gossip_update(self_node_id, ctx.default_ttl, key, value, false, &ctx.hlc);
    apply_and_notify(&ctx.kv_state, &update); // apply, then persist (persistence.rs invariant 1)
    if let Some(wal) = ctx.wal.get() {
        wal.append_try(crate::framing::sync_entry_from(&update));
    }
    dispatch_gossip_try_send(
        &ctx.gossip_txs, crate::framing::local_kv_wire_msg(ctx, update),
        self_node_id.id_hash(), ForwardHint::All, &ctx.kv_state.dropped_frames,
    )
}

/// Opens a mailbox for events of `kind` addressed to `self_node_id`.
///
/// Free-function variant of [`ServiceHandle::open_mailbox`] for callers that
/// hold only `Arc<TaskCtx>`. The `shutdown_rx` must come from the owning
/// agent's broadcast (available on `HttpCtx::shutdown_rx`).
pub(crate) fn open_mailbox_ctx(
    ctx:          Arc<TaskCtx>,
    self_node_id: &NodeId,
    kind:         Arc<str>,
    capacity:     usize,
    shutdown_rx:  tokio::sync::watch::Receiver<bool>,
) -> (MailboxHandle, mpsc::Receiver<MeshEvent>) {
    let prefix: Arc<str> = Arc::from(mailbox_prefix(self_node_id, &kind).as_str());
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let (tx, rx)               = mpsc::channel(capacity);
    let watcher = super::capability_ops::subscribe_prefix_on_kv(
        &ctx.kv_state, Arc::clone(&prefix),
    );
    tokio::spawn(mailbox_task(ctx, prefix, kind, Sink::Immediate(tx), cancel_rx, shutdown_rx, watcher));
    (MailboxHandle { _cancel: cancel_tx }, rx)
}

/// [`open_mailbox_ctx`] for a consumer that says when it has handed an event on: each entry is
/// deleted when its [`AckedEvent::ack`] is acked, never when it enters the channel (post-360
/// hardening row E — the gateway's mailbox stream). Entries handed out and not yet acked are not handed
/// out again by **any** acked stream of this `(node, kind)` — the in-flight set is shared (#613's
/// review); one dropped un-acked is released to the others and to the next stream opened.
#[cfg_attr(not(feature = "gateway"), allow(dead_code))] // the gateway's mailbox stream is the one consumer
pub(crate) fn open_mailbox_acked(
    ctx:          Arc<TaskCtx>,
    self_node_id: &NodeId,
    kind:         Arc<str>,
    capacity:     usize,
    shutdown_rx:  tokio::sync::watch::Receiver<bool>,
) -> (MailboxHandle, mpsc::Receiver<AckedEvent>) {
    let prefix: Arc<str> = Arc::from(mailbox_prefix(self_node_id, &kind).as_str());
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let (tx, rx)               = mpsc::channel(capacity);
    let watcher = super::capability_ops::subscribe_prefix_on_kv(
        &ctx.kv_state, Arc::clone(&prefix),
    );
    let sink = Sink::Acked { tx, in_flight: shared_in_flight(&ctx.mailbox_in_flight, &prefix) };
    tokio::spawn(mailbox_task(ctx, prefix, kind, sink, cancel_rx, shutdown_rx, watcher));
    (MailboxHandle { _cancel: cancel_tx }, rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GossipAgent, GossipConfig, NodeId};
    use bytes::Bytes;
    use std::{sync::Arc, time::Duration};

    fn alloc_port() -> u16 { crate::test_util::alloc_port() }

    #[tokio::test]
    async fn deliver_and_open_mailbox_loopback() {
        let port = alloc_port();
        let id   = NodeId::new("127.0.0.1", port).unwrap();
        let mut cfg = GossipConfig::default(); cfg.bind_port = port;
        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();

        let (handle, mut rx) = agent.service().open_mailbox("test", 16);

        let delivered = agent.service().deliver_event(&id, "test", Bytes::from_static(b"hello-mailbox"));
        assert!(delivered, "deliver_event returned false");

        // The watcher fires immediately since we're local.
        let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout waiting for mailbox event")
            .expect("channel closed");

        assert_eq!(event.payload, Bytes::from_static(b"hello-mailbox"));
        assert_eq!(event.sender, id);
        assert_eq!(event.kind.as_ref(), "test");

        // Entry should now be tombstoned — second poll should be empty.
        drop(handle);
        agent.shutdown().await;
    }

    #[tokio::test]
    async fn mailbox_hlc_ordering() {
        let port = alloc_port();
        let id   = NodeId::new("127.0.0.1", port).unwrap();
        let mut cfg = GossipConfig::default(); cfg.bind_port = port;
        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();

        let (handle, mut rx) = agent.service().open_mailbox("order-test", 16);

        // Deliver three events in sequence — HLC guarantees key-order = causal order.
        for i in 0u8..3 {
            agent.service().deliver_event(&id, "order-test", Bytes::from(vec![i]));
        }

        let mut received = Vec::new();
        for _ in 0..3 {
            let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await.unwrap().unwrap();
            received.push(event.payload[0]);
        }
        assert_eq!(received, vec![0, 1, 2], "events not in causal order");

        drop(handle);
        agent.shutdown().await;
    }

    /// Delivers more than DRAIN_CHUNK events in one shot to verify chunked drain
    /// processes all of them in causal order without blocking.
    #[tokio::test]
    async fn drain_large_burst_in_order() {
        let port = alloc_port();
        let id   = NodeId::new("127.0.0.1", port).unwrap();
        let mut cfg = GossipConfig::default(); cfg.bind_port = port;
        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();

        // Use a channel large enough to hold all events without back-pressure.
        let n: usize = DRAIN_CHUNK * 3;
        let (handle, mut rx) = agent.service().open_mailbox("burst-test", n + 16);

        // Deliver n events before opening the watcher — they land in KV first.
        for i in 0..n {
            agent.service().deliver_event(&id, "burst-test", Bytes::from(i.to_be_bytes().to_vec()));
        }

        let mut received: Vec<usize> = Vec::with_capacity(n);
        for _ in 0..n {
            let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await.expect("timeout").expect("channel closed");
            let arr: [u8; 8] = event.payload.as_ref().try_into().unwrap();
            received.push(usize::from_be_bytes(arr));
        }

        let expected: Vec<usize> = (0..n).collect();
        assert_eq!(received, expected, "burst events not delivered in causal order");

        drop(handle);
        agent.shutdown().await;
    }

    // ── ER1 (R7-01): a mailbox key names its sender ─────────────────────────────

    async fn solo_agent() -> Arc<GossipAgent> {
        let port = alloc_port();
        let mut cfg = GossipConfig::default(); cfg.bind_port = port;
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        agent.start().await.unwrap();
        agent
    }

    /// Receives up to `n` events, each within two seconds; fewer when the mailbox runs dry.
    async fn recv_up_to(rx: &mut mpsc::Receiver<MeshEvent>, n: usize) -> Vec<MeshEvent> {
        let mut out = Vec::new();
        while out.len() < n {
            match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                Ok(Some(ev)) => out.push(ev),
                _ => break,
            }
        }
        out
    }

    /// Writes a raw entry under `key`, as an older (or a foreign) writer would have.
    fn put_raw(ctx: &Arc<TaskCtx>, key: String, sender: &NodeId, payload: &'static [u8]) {
        let update = make_gossip_update(
            sender, ctx.default_ttl, Arc::from(key.as_str()), encode_value(sender, &Bytes::from_static(payload)), false, &ctx.hlc,
        );
        apply_and_notify(&ctx.kv_state, &update);
    }

    fn senders_and_payloads(events: &[MeshEvent]) -> std::collections::BTreeSet<(String, Vec<u8>)> {
        events.iter().map(|e| (e.sender.to_string(), e.payload.to_vec())).collect()
    }

    /// Two senders whose clocks hand out one stamp wrote one key, and LWW kept one of the two events
    /// while both calls returned `true`. The key now names its sender, so both are kept.
    #[tokio::test]
    async fn two_senders_at_one_stamp_are_both_delivered() {
        let agent = solo_agent().await;
        let id = agent.node_id().clone();
        let other = NodeId::new("127.0.0.1", 1).unwrap();
        let ts = agent.task_ctx.hlc.tick();
        assert!(deliver_event_at(&agent.task_ctx, &id, &id, Arc::from("pair"), Bytes::from_static(b"first"), ts));
        assert!(deliver_event_at(&agent.task_ctx, &other, &id, Arc::from("pair"), Bytes::from_static(b"second"), ts));
        let (handle, mut rx) = agent.service().open_mailbox("pair", 8);
        let got = recv_up_to(&mut rx, 2).await;
        assert_eq!(got.len(), 2, "two accepted deliveries at one stamp, {} delivered", got.len());
        assert_eq!(senders_and_payloads(&got), [(id.to_string(), b"first".to_vec()), (other.to_string(), b"second".to_vec())].into());
        assert!(got.iter().all(|e| e.hlc_ts == ts), "both carry the stamp they were delivered at");
        drop(handle);
        agent.shutdown().await;
    }

    /// The reviewer's sequence (Round 7, issue 01): two connected nodes deliver to one recipient and kind
    /// in one millisecond with one logical count, so both write one key. Sampled until the two clocks
    /// agree, then delivered through the seam at that stamp; after convergence the recipient drains both.
    #[tokio::test]
    async fn two_nodes_delivering_in_one_millisecond_lose_nothing() {
        let one = solo_agent().await;
        let port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers.push(one.node_id().clone());
        let two = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        two.start().await.unwrap();
        let (a, b) = (one.node_id().clone(), two.node_id().clone());
        for _ in 0..400 {
            if one.connected_peers().contains(&b) && two.connected_peers().contains(&a) { break; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(one.connected_peers().contains(&b) && two.connected_peers().contains(&a), "the two nodes never connected");

        let ts = (0..100_000).find_map(|_| {
            let (x, y) = (one.task_ctx.hlc.tick(), two.task_ctx.hlc.tick());
            (x == y).then_some(x)
        }).expect("the two clocks never handed out one stamp");
        assert!(deliver_event_at(&one.task_ctx, &a, &a, Arc::from("both"), Bytes::from_static(b"first"), ts));
        assert!(deliver_event_at(&two.task_ctx, &b, &a, Arc::from("both"), Bytes::from_static(b"second"), ts));
        // Converged: each node holds what the other wrote, and the two views agree.
        assert!(two.kv().set_async("er1/fence-two", "two").await);
        assert!(one.kv().set_async("er1/fence-one", "one").await);
        let prefix = format!("mailbox/{a}/both/");
        let converged = || {
            let (x, y) = (one.kv().scan_prefix(&prefix), two.kv().scan_prefix(&prefix));
            one.kv().get("er1/fence-two").is_some() && two.kv().get("er1/fence-one").is_some() && !x.is_empty() && x == y
        };
        for _ in 0..400 {
            if converged() { break; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(converged(), "the two nodes never converged");
        let (handle, mut rx) = one.service().open_mailbox("both", 8);
        let got = recv_up_to(&mut rx, 2).await;
        assert_eq!(senders_and_payloads(&got), [(a.to_string(), b"first".to_vec()), (b.to_string(), b"second".to_vec())].into(),
            "both nodes' events reach the recipient");
        drop(handle);
        one.shutdown().await;
        two.shutdown().await;
    }

    /// The same collision through the gateway's acked mailbox: two events, each acked.
    #[tokio::test]
    async fn two_senders_at_one_stamp_through_the_acked_mailbox() {
        let agent = solo_agent().await;
        let id = agent.node_id().clone();
        let other = NodeId::new("127.0.0.1", 1).unwrap();
        let ts = agent.task_ctx.hlc.tick();
        assert!(deliver_event_at(&agent.task_ctx, &id, &id, Arc::from("acked"), Bytes::from_static(b"first"), ts));
        assert!(deliver_event_at(&agent.task_ctx, &other, &id, Arc::from("acked"), Bytes::from_static(b"second"), ts));
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let (handle, mut rx) = open_mailbox_acked(Arc::clone(&agent.task_ctx), &id, Arc::from("acked"), 8, shutdown_rx);
        let mut got = Vec::new();
        while got.len() < 2 {
            match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                Ok(Some(AckedEvent { event, ack })) => { ack.ack(); got.push(event); }
                _ => break,
            }
        }
        assert_eq!(senders_and_payloads(&got), [(id.to_string(), b"first".to_vec()), (other.to_string(), b"second".to_vec())].into());
        drop(handle);
        agent.shutdown().await;
    }

    /// A salted entry (`{hlc:016x}/{sender}`) carries its stamp. The old reader parsed the whole suffix as
    /// hex and fell back to 0.
    #[tokio::test]
    async fn a_new_shape_entry_carries_its_stamp() {
        let agent = solo_agent().await;
        let id = agent.node_id().clone();
        let ts = agent.task_ctx.hlc.tick();
        put_raw(&agent.task_ctx, format!("mailbox/{id}/stamped/{ts:016x}/{id}"), &id, b"salted");
        let (handle, mut rx) = agent.service().open_mailbox("stamped", 8);
        let got = recv_up_to(&mut rx, 1).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].payload.as_ref(), b"salted");
        assert_eq!(got[0].hlc_ts, ts, "the stamp is the key's first segment");
        drop(handle);
        agent.shutdown().await;
    }

    /// Mailbox `a` scans `mailbox/{t}/a/`, which also holds kind `a/b`'s entries. One whose suffix is
    /// neither `{hlc:016x}` nor `{hlc:016x}/{sender}` is not this mailbox's: it is not delivered, not
    /// tombstoned, and the drain still ends (the filter sits before the emptiness check).
    #[tokio::test]
    async fn a_suffix_that_is_neither_shape_is_left_alone_and_the_drain_ends() {
        let agent = solo_agent().await;
        let id = agent.node_id().clone();
        let ctx = Arc::clone(&agent.task_ctx);
        assert!(agent.service().deliver_event(&id, "a/b", Bytes::from_static(b"for a/b")));
        let foreign_prefix = format!("mailbox/{id}/a/b/");
        assert_eq!(agent.kv().scan_prefix(&foreign_prefix).len(), 1);

        let prefix: Arc<str> = Arc::from(format!("mailbox/{id}/a/").as_str());
        let kind: Arc<str> = Arc::from("a");
        let (tx, mut rx) = mpsc::channel(8);
        let sink = Sink::Immediate(tx);
        tokio::time::timeout(Duration::from_secs(2), drain_prefix(&ctx, &prefix, &kind, &sink))
            .await.expect("the drain ends with a foreign entry under its prefix");
        assert!(rx.try_recv().is_err(), "kind a/b's event was delivered to mailbox a");
        assert_eq!(agent.kv().scan_prefix(&foreign_prefix).len(), 1, "kind a/b's entry was tombstoned by mailbox a");

        // Mailbox `a`'s own events still drain, past the foreign entry; and `a/b`'s reaches its own mailbox.
        assert!(agent.service().deliver_event(&id, "a", Bytes::from_static(b"for a")));
        tokio::time::timeout(Duration::from_secs(2), drain_prefix(&ctx, &prefix, &kind, &sink)).await.expect("the drain ends");
        let own = rx.try_recv().expect("mailbox a's own event");
        assert_eq!(own.payload.as_ref(), b"for a");
        assert!(rx.try_recv().is_err());
        let (handle, mut ab) = agent.service().open_mailbox("a/b", 8);
        let got = recv_up_to(&mut ab, 1).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].payload.as_ref(), b"for a/b");
        assert_ne!(got[0].hlc_ts, 0);
        drop(handle);
        agent.shutdown().await;
    }

    /// Control for the new parse: an entry in the 2.40 shape (`{hlc:016x}`, no sender) drains with its stamp.
    #[tokio::test]
    async fn an_old_shape_entry_is_drained_with_its_stamp() {
        let agent = solo_agent().await;
        let id = agent.node_id().clone();
        let ts = agent.task_ctx.hlc.tick();
        put_raw(&agent.task_ctx, format!("mailbox/{id}/legacy/{ts:016x}"), &id, b"old");
        let (handle, mut rx) = agent.service().open_mailbox("legacy", 8);
        let got = recv_up_to(&mut rx, 1).await;
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].payload.as_ref(), got[0].hlc_ts), (b"old".as_slice(), ts));
        drop(handle);
        agent.shutdown().await;
    }

    /// Control: an old-shape and a salted entry at one stamp are both delivered, the old one first (a
    /// shorter key sorts first at an equal stamp), whichever was written first.
    #[tokio::test]
    async fn old_and_new_shapes_at_one_stamp_are_delivered_in_key_order() {
        let agent = solo_agent().await;
        let id = agent.node_id().clone();
        let ts = agent.task_ctx.hlc.tick();
        put_raw(&agent.task_ctx, format!("mailbox/{id}/mixed/{ts:016x}/{id}"), &id, b"new");
        put_raw(&agent.task_ctx, format!("mailbox/{id}/mixed/{ts:016x}"), &id, b"old");
        let (handle, mut rx) = agent.service().open_mailbox("mixed", 8);
        let got = recv_up_to(&mut rx, 2).await;
        let order: Vec<&[u8]> = got.iter().map(|e| e.payload.as_ref()).collect();
        assert_eq!(order, vec![b"old".as_slice(), b"new".as_slice()]);
        drop(handle);
        agent.shutdown().await;
    }

    /// The door (`deliver_event_in`) reaches the same writer: its key names the sender.
    #[tokio::test]
    async fn deliver_event_in_writes_the_salted_key() {
        let agent = solo_agent().await;
        let id = agent.node_id().clone();
        let ok = agent.service()
            .deliver_event_in(&id, "door", Bytes::from_static(b"x"), &crate::agent::causal::CausalFields::default())
            .await.expect("no exposure rule: not refused");
        assert!(ok);
        let prefix = format!("mailbox/{id}/door/");
        let rows = agent.kv().scan_prefix(&prefix);
        assert_eq!(rows.len(), 1);
        let suffix = rows[0].0.strip_prefix(prefix.as_str()).unwrap().to_string();
        let (hlc, sender) = suffix.split_once('/').expect("the key has a sender segment");
        assert_eq!(hlc.len(), 16, "{suffix}");
        assert_eq!(sender, id.to_string());
        agent.shutdown().await;
    }

    /// The strict suffix parse: the two shapes and nothing else.
    #[test]
    fn the_suffix_parse_accepts_exactly_the_two_shapes() {
        assert_eq!(parse_suffix("00000000000000ff"), Some(0xff));
        assert_eq!(parse_suffix("00000000000000ff/127.0.0.1:9000"), Some(0xff));
        for neither in ["", "ff", "00000000000000ff/", "00000000000000ff/a/b", "b/00000000000000ff",
                        "b/00000000000000ff/127.0.0.1:9000", "0000000000000000ff", "00000000000000fg"] {
            assert_eq!(parse_suffix(neither), None, "{neither:?}");
        }
        let (t, s) = (NodeId::new("127.0.0.1", 1).unwrap(), NodeId::new("127.0.0.1", 2).unwrap());
        let key = mailbox_key(&t, "k", 0xff, &s);
        assert_eq!(parse_suffix(key.strip_prefix(mailbox_prefix(&t, "k").as_str()).unwrap()), Some(0xff));
    }

    #[test]
    fn encode_decode_roundtrip() {
        let id      = NodeId::new("127.0.0.1", 12345).unwrap();
        let payload = Bytes::from_static(b"roundtrip");
        let encoded = encode_value(&id, &payload);
        let (sender, got) = decode_value(&encoded).unwrap();
        assert_eq!(sender, id);
        assert_eq!(got, payload);
    }
}
