//! Actor/Event mailboxes — KV-backed durable event delivery.
//!
//! [`ServiceHandle::deliver_event`] writes an event into the mesh at a
//! HLC-ordered key under `mailbox/{target}/{kind}/{hlc_hex}`. Any node
//! that knows the target's [`NodeId`] can deliver; the event propagates
//! via the normal gossip KV path. The target's [`open_mailbox`] watcher
//! picks up new entries in causal order, delivers them to a channel, and
//! tombstones each entry so it is not redelivered after restart.
//!
//! ## Delivery guarantees
//!
//! - **At-least-once** within the gossip TTL window. Events stored in KV
//!   propagate to all live nodes via anti-entropy; a crashed target node
//!   will receive pending events on reconnect as long as they have not
//!   yet expired (default TTL: 5 minutes).
//! - **Causal order** within the watcher task: entries are sorted by their
//!   HLC key before delivery, so causal happens-before is preserved.
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
//! `mailbox/{target_node_id}/{kind}/{hlc_ts:016x}` → `sender_node_id_len(2 LE) | sender_bytes | payload`

use crate::framing::{ForwardHint, WireMessage, dispatch_gossip_try_send, make_gossip_update};
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
/// store; once the mailbox itself is dropped, the next one opened delivers it again.
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
}

/// The keys an acked mailbox has handed to its consumer and not yet had acknowledged: the drain skips
/// them so an entry is never queued twice while its first delivery is outstanding.
type InFlight = Arc<papaya::HashMap<Arc<str>, ()>>;

#[cfg_attr(not(feature = "gateway"), allow(dead_code))] // the gateway's mailbox stream is the one consumer
impl MailboxAck {
    /// The consumer has the event: delete its entry. The tombstone lands **before** the key leaves
    /// the in-flight set, so a drain that no longer sees the key in flight sees it deleted
    /// (`drain_prefix` checks in that order).
    pub(crate) fn ack(self) {
        tombstone(&self.ctx, Arc::clone(&self.key));
        self.in_flight.pin().remove(&self.key);
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

fn tombstone(ctx: &Arc<TaskCtx>, key: Arc<str>) {
    let update = make_gossip_update(
        &ctx.node_id, ctx.default_ttl, key, Bytes::new(), true, &ctx.hlc,
    );
    apply_and_notify(&ctx.kv_state, &update);
    dispatch_gossip_try_send(
        &ctx.gossip_txs,
        WireMessage::Data(update),
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

/// Delivers up to `DRAIN_CHUNK` mailbox entries in HLC key order, tombstoning
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
        let mut entries = scan_kv_prefix(&ctx.kv_state, prefix.as_ref());
        if let Sink::Acked { in_flight, .. } = sink {
            // Handed on and not yet acked: not this drain's to send again.
            let pinned = in_flight.pin();
            entries.retain(|(key, _)| !pinned.contains_key(key));
        }
        if entries.is_empty() { break; }
        // sort_unstable is fine — keys are unique HLC timestamps.
        entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));

        let chunk: Vec<_> = entries.drain(..entries.len().min(DRAIN_CHUNK)).collect();
        for (key, value) in chunk {
            let Some((sender, payload)) = decode_value(&value) else {
                tombstone(ctx, key);  // malformed — evict silently
                continue;
            };
            let ts_hex = key.strip_prefix(prefix.as_ref()).unwrap_or("");
            let hlc_ts = u64::from_str_radix(ts_hex, 16).unwrap_or(0);
            let event  = MeshEvent { kind: Arc::clone(kind), sender, payload, hlc_ts };
            match sink {
                Sink::Immediate(tx) => {
                    if tx.send(event).await.is_err() {
                        return; // receiver dropped
                    }
                    tombstone(ctx, key);
                }
                Sink::Acked { tx, in_flight } => {
                    // The scan above may predate an ack that has since run: an entry acked after the
                    // scan was tombstoned before it left the in-flight set, so re-check both, in the
                    // order `MailboxAck::ack` writes them.
                    if in_flight.pin().contains_key(&key) { continue; }
                    if ctx.kv_state.store.pin().get(key.as_ref()).is_none_or(|e| e.data.is_none()) { continue; }
                    in_flight.pin().insert(Arc::clone(&key), ());
                    let ack = MailboxAck { ctx: Arc::clone(ctx), key, in_flight: Arc::clone(in_flight) };
                    if tx.send(AckedEvent { event, ack }).await.is_err() {
                        return; // receiver dropped: the entry stays for the next mailbox
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
    let ts  = ctx.hlc.tick();
    let key: Arc<str> = Arc::from(
        format!("mailbox/{}/{}/{:016x}", target, kind, ts).as_str(),
    );
    let value = encode_value(self_node_id, &payload);
    let update = make_gossip_update(self_node_id, ctx.default_ttl, key, value, false, &ctx.hlc);
    apply_and_notify(&ctx.kv_state, &update); // apply, then persist (persistence.rs invariant 1)
    if let Some(wal) = ctx.wal.get() {
        wal.append_try(crate::framing::sync_entry_from(&update));
    }
    dispatch_gossip_try_send(
        &ctx.gossip_txs, WireMessage::Data(update),
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
    let prefix: Arc<str> = Arc::from(
        format!("mailbox/{}/{}/", self_node_id, kind).as_str(),
    );
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
/// hardening row E — the gateway's mailbox stream). Entries handed on and not yet acked are not
/// queued again while this mailbox lives; once it is dropped, they are the next mailbox's.
#[cfg_attr(not(feature = "gateway"), allow(dead_code))] // the gateway's mailbox stream is the one consumer
pub(crate) fn open_mailbox_acked(
    ctx:          Arc<TaskCtx>,
    self_node_id: &NodeId,
    kind:         Arc<str>,
    capacity:     usize,
    shutdown_rx:  tokio::sync::watch::Receiver<bool>,
) -> (MailboxHandle, mpsc::Receiver<AckedEvent>) {
    let prefix: Arc<str> = Arc::from(
        format!("mailbox/{}/{}/", self_node_id, kind).as_str(),
    );
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let (tx, rx)               = mpsc::channel(capacity);
    let watcher = super::capability_ops::subscribe_prefix_on_kv(
        &ctx.kv_state, Arc::clone(&prefix),
    );
    let sink = Sink::Acked { tx, in_flight: Arc::new(papaya::HashMap::new()) };
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
