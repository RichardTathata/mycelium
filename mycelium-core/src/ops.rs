//! Free operations over [`CoreCtx`] — the implementation behind the Layer-I/II typed
//! handles (`KvHandle`, `MeshHandle`, `SchemaHandle`).
//!
//! These were `agent::helpers` functions over `&TaskCtx` in the full crate; M3 moved them
//! here over `&CoreCtx` so the substrate handles can live in `mycelium-core`. Upper call
//! sites pass `&TaskCtx`, which Deref-coerces to `&CoreCtx`. `emit_signal*`'s former local
//! `rpc_pending` fast-path is now [`CoreCtx::reply_interceptor`](crate::CoreCtx) — the RPC
//! correlation closure the upper service layer registers (mechanism in core; agency above).

use crate::context::CoreCtx;
use crate::framing::{
    dispatch_gossip_send, dispatch_gossip_try_send, make_gossip_update, make_gossip_update_stamped, make_kv_wire_msg,
    sync_entry_from, ForwardHint, WireMessage,
};
use crate::signal::{Boundary, Signal, SignalHandlers, SignalScope};
use crate::store::apply_and_notify;
use bytes::Bytes;
use parking_lot::RwLock;
use std::sync::Arc;

/// The inventory's named RNG streams (§2.2). Declared once so a trace written today and a reader
/// written later cannot disagree about which stream a draw came from.
const NONCE_STREAM: &str = "nonce";
/// The opacity shedding roll's stream — separate from `nonce`, so adding a nonce draw does not
/// move a shedding decision.
const SHED_STREAM: &str = "shed";

/// Checks the boundary and opacity gate, then delivers `signal` locally.
/// `combined_fill` is `max(handler_fill, shard_fill)` — pre-computed by the caller.
fn deliver_locally(
    signal_boundary: &RwLock<Boundary>,
    signal_handlers: &SignalHandlers,
    signal: &Signal,
    combined_fill: f32,
) {
    if !signal_boundary.read().admits(&signal.scope) { return; }
    let admit = match &signal.scope {
        SignalScope::Individual(_) => true,
        // Boundary-transition announcements are control-plane, not work. Shedding the very
        // signal that says "I'm now shedding" is self-defeating — and it fires *only* when
        // `combined_fill > 0` (under load), the one moment it must get through. A shed here is a
        // permanent miss (the governor emits each transition once), so a local subscriber can miss
        // the transition under load. Exempt them from the probabilistic shed, like `Individual`.
        _ if matches!(
            signal.kind.as_ref(),
            crate::signal::signal_kind::BOUNDARY_OPAQUE
                | crate::signal::signal_kind::BOUNDARY_TRANSPARENT
        ) => true,
        _ => combined_fill == 0.0 || crate::sim_seam::rng_f32(SHED_STREAM) >= combined_fill,
    };
    if admit {
        signal_handlers.deliver(signal);
    }
}

/// Generates a nonce, marks it seen, delivers locally (boundary + opacity checks),
/// encodes the wire frame, and routes to the correct gossip shard via `try_send`.
pub fn emit_signal(
    ctx:     &CoreCtx,
    kind:    Arc<str>,
    scope:   SignalScope,
    payload: Bytes,
) -> bool {
    let nonce = crate::sim_seam::rng_u64_from(NONCE_STREAM, 1);
    let ts = crate::hlc::physical_ms(ctx.hlc.current()); // C11: dedup TTL, fails closed
    ctx.seen.mark_and_check(nonce, ts);
    let sig = Signal {
        kind: Arc::clone(&kind), scope: scope.clone(),
        payload, sender: ctx.node_id.clone(), nonce,
    };
    // Co-located rpc.result / bulk.result: the upper-registered interceptor claims the
    // reply (fires the waiting oneshot) and we skip the signal_handlers fan-out.
    let nonce_claimed = match ctx.reply_interceptor.as_ref() {
        Some(claim) => claim(&sig),
        None => false,
    };
    if !nonce_claimed {
        let handler_fill = ctx.signal_handlers.fill_ratio(&kind);
        let combined = handler_fill.max(crate::framing::gossip_shard_fill(&ctx.gossip_txs));
        deliver_locally(&ctx.signal_boundary, &ctx.signal_handlers, &sig, combined);
    }
    let hint = forward_hint(&sig.scope);
    #[cfg(feature = "metrics")]
    {
        let scope_label = scope_label(&sig.scope);
        metrics::counter!("gossip_signals_emitted_total", "scope" => scope_label).increment(1);
    }
    dispatch_gossip_try_send(
        &ctx.gossip_txs,
        WireMessage::Signal { ttl: ctx.default_ttl, nonce, sender: ctx.node_id.clone(), scope: sig.scope.clone(), kind: sig.kind.clone(), payload: sig.payload.clone(), hlc_seq: None },
        ctx.node_id.id_hash(), hint, &ctx.kv_state.dropped_frames,
    )
}

/// Like [`emit_signal`] but stamps an HLC sequence number for ordered delivery.
pub fn emit_signal_ordered(
    ctx:     &CoreCtx,
    kind:    Arc<str>,
    scope:   SignalScope,
    payload: Bytes,
) -> bool {
    let nonce   = crate::sim_seam::rng_u64_from(NONCE_STREAM, 1);
    let ts      = crate::hlc::physical_ms(ctx.hlc.current()); // C11: dedup TTL, fails closed
    let hlc_seq = ctx.hlc.tick();
    ctx.seen.mark_and_check(nonce, ts);
    let sig = Signal {
        kind: Arc::clone(&kind), scope: scope.clone(),
        payload, sender: ctx.node_id.clone(), nonce,
    };
    let nonce_claimed = match ctx.reply_interceptor.as_ref() {
        Some(claim) => claim(&sig),
        None => false,
    };
    if !nonce_claimed {
        let handler_fill = ctx.signal_handlers.fill_ratio(&kind);
        let combined = handler_fill.max(crate::framing::gossip_shard_fill(&ctx.gossip_txs));
        deliver_locally(&ctx.signal_boundary, &ctx.signal_handlers, &sig, combined);
    }
    let hint = forward_hint(&sig.scope);
    dispatch_gossip_try_send(
        &ctx.gossip_txs,
        WireMessage::Signal {
            ttl: ctx.default_ttl, nonce, sender: ctx.node_id.clone(),
            scope: sig.scope.clone(), kind: sig.kind.clone(), payload: sig.payload.clone(), hlc_seq: Some(hlc_seq),
        },
        ctx.node_id.id_hash(), hint, &ctx.kv_state.dropped_frames,
    )
}

/// Like [`emit_signal`] but awaits gossip channel capacity instead of dropping.
pub async fn emit_signal_async(
    ctx:     &CoreCtx,
    kind:    Arc<str>,
    scope:   SignalScope,
    payload: Bytes,
) -> bool {
    let nonce = crate::sim_seam::rng_u64_from(NONCE_STREAM, 1);
    let ts = crate::hlc::physical_ms(ctx.hlc.current()); // C11: dedup TTL, fails closed
    ctx.seen.mark_and_check(nonce, ts);
    let handler_fill = ctx.signal_handlers.fill_ratio(&kind);
    let combined = handler_fill.max(crate::framing::gossip_shard_fill(&ctx.gossip_txs));
    deliver_locally(&ctx.signal_boundary, &ctx.signal_handlers, &Signal {
        kind: Arc::clone(&kind), scope: scope.clone(),
        payload: payload.clone(), sender: ctx.node_id.clone(), nonce,
    }, combined);
    let hint = forward_hint(&scope);
    dispatch_gossip_send(
        &ctx.gossip_txs,
        WireMessage::Signal { ttl: ctx.default_ttl, nonce, sender: ctx.node_id.clone(), scope, kind, payload, hlc_seq: None },
        ctx.node_id.id_hash(), hint,
    ).await
}

fn forward_hint(scope: &SignalScope) -> ForwardHint {
    match scope {
        SignalScope::Cluster           => ForwardHint::All,
        SignalScope::Group(name)      => ForwardHint::Group(Arc::clone(name)),
        SignalScope::Individual(peer) => ForwardHint::Individual(peer.clone()),
        SignalScope::Groups(_)        => ForwardHint::All,
    }
}

#[cfg(feature = "metrics")]
fn scope_label(scope: &SignalScope) -> &'static str {
    match scope {
        SignalScope::Cluster        => "system",
        SignalScope::Group(_)      => "group",
        SignalScope::Individual(_) => "node",
        SignalScope::Groups(_)     => "groups",
    }
}

// ── KV primitives usable from typed sub-handles ───────────────────────────────

/// Returns the current value for `key`, or `None` if absent or tombstoned.
pub fn kv_get(ctx: &CoreCtx, key: &str) -> Option<Bytes> {
    ctx.kv_state.store.pin().get(key).and_then(|e| e.data.clone())
}

/// Subscribes to changes for `key`.
pub fn kv_subscribe(ctx: &CoreCtx, key: impl Into<Arc<str>>) -> tokio::sync::watch::Receiver<Option<Bytes>> {
    let key_arc: Arc<str> = key.into();
    loop {
        let guard = ctx.kv_state.subscriptions.pin();
        if let Some(tx) = guard.get(&key_arc)
            && !tx.is_closed() { return tx.subscribe(); }
        let current = ctx.kv_state.store.pin().get(&*key_arc).and_then(|e| e.data.clone());
        let (new_tx, rx) = tokio::sync::watch::channel(current);
        let mut slot = Some(new_tx);
        let result = guard.compute(Arc::clone(&key_arc), |existing| match existing {
            Some((_, tx)) if !tx.is_closed() => papaya::Operation::Abort(()),
            _ => match slot.take() {
                Some(tx) => papaya::Operation::Insert(tx),
                None     => papaya::Operation::Abort(()),
            },
        });
        if matches!(result, papaya::Compute::Inserted(..) | papaya::Compute::Updated { .. }) {
            return rx;
        }
    }
}

/// Subscribes to any write touching keys under `prefix`.
pub fn kv_subscribe_prefix(ctx: &CoreCtx, prefix: impl Into<Arc<str>>) -> tokio::sync::watch::Receiver<u64> {
    let prefix_arc: Arc<str> = prefix.into();
    loop {
        let guard = ctx.kv_state.prefix_watchers.pin();
        if let Some(tx) = guard.get(&prefix_arc)
            && !tx.is_closed() { return tx.subscribe(); }
        let (new_tx, rx) = tokio::sync::watch::channel(0u64);
        let new_tx_arc   = std::sync::Arc::new(new_tx);
        let mut slot     = Some(new_tx_arc);
        let result = guard.compute(Arc::clone(&prefix_arc), |existing| match existing {
            Some((_, tx)) if !tx.is_closed() => papaya::Operation::Abort(()),
            _ => match slot.take() {
                Some(tx) => papaya::Operation::Insert(tx),
                None     => papaya::Operation::Abort(()),
            },
        });
        if matches!(result, papaya::Compute::Inserted(..) | papaya::Compute::Updated { .. }) {
            return rx;
        }
    }
}

/// Per-subscriber variant: fires only when the prefix matches AND `predicate(&key)` is true.
pub fn kv_subscribe_prefix_with_predicate<P, F>(
    ctx:       &CoreCtx,
    prefix:    P,
    predicate: F,
) -> tokio::sync::watch::Receiver<u64>
where
    P: Into<Arc<str>>,
    F: Fn(&str) -> bool + Send + Sync + 'static,
{
    use std::sync::atomic::Ordering;
    let prefix_arc: Arc<str> = prefix.into();
    let (tx, rx) = tokio::sync::watch::channel(0u64);
    let entry = crate::store::PrefixPredicateWatcher {
        prefix:    prefix_arc,
        predicate: Arc::new(predicate),
        tx:        Arc::new(tx),
    };
    let id = ctx.kv_state.next_pred_watcher_id.fetch_add(1, Ordering::Relaxed);
    ctx.kv_state.prefix_predicate_watchers.pin().insert(id, entry);
    rx
}

/// Returns all live key-value pairs whose key starts with `prefix`.
pub fn kv_scan_prefix(ctx: &CoreCtx, prefix: &str) -> Vec<(Arc<str>, Bytes)> {
    crate::store::scan_kv_prefix(&ctx.kv_state, prefix)
}

/// Rejects a write whose `key + value` cannot fit a single gossip frame. Accepting it
/// would apply locally (and WAL) but never propagate: the per-peer writer cannot frame
/// it and anti-entropy skips entries that don't fit — silent permanent divergence.
/// Returns `true` (= reject) after a `warn!`; callers surface it as `false` ("not queued").
fn reject_oversized_write(key: &str, value_len: usize) -> bool {
    let size = key.len() + value_len;
    if size <= crate::framing::MAX_KV_WRITE_BYTES {
        return false;
    }
    tracing::warn!(
        key, size, limit = crate::framing::MAX_KV_WRITE_BYTES,
        "kv write rejected: key + value cannot fit a gossip frame and would silently \
         diverge; use the bulk transport (bulk_call / bulk_serve) for large payloads"
    );
    true
}

/// Stores `value` under `key`, applies locally, queues WAL (try-send), gossips (try-send).
///
/// Returns `false` without applying anything if `key.len() + value.len()` exceeds
/// [`MAX_KV_WRITE_BYTES`](crate::framing::MAX_KV_WRITE_BYTES) — such a write cannot be
/// encoded into one gossip frame and would otherwise diverge silently.
pub fn kv_set(ctx: &CoreCtx, key: Arc<str>, value: Bytes) -> bool {
    if reject_oversized_write(&key, value.len()) {
        return false;
    }
    let update = make_gossip_update(&ctx.node_id, ctx.default_ttl, key, value, false, &ctx.hlc);
    // Apply first, then persist: the store is never behind the WAL, so a writer-side
    // snapshot scan always contains every record the writer has been handed
    // (persistence.rs durability invariant 1).
    apply_and_notify(&ctx.kv_state, &update);
    if let Some(wal) = ctx.wal.get() {
        wal.append_try(sync_entry_from(&update));
    }
    #[cfg(feature = "metrics")]
    metrics::counter!("gossip_kv_writes_total").increment(1);
    let tls = ctx.tls.get().map(Arc::as_ref);
    let msg = make_kv_wire_msg(update, ctx.node_id.id_hash(), tls);
    dispatch_gossip_try_send(
        &ctx.gossip_txs, msg,
        ctx.node_id.id_hash(), ForwardHint::All, &ctx.kv_state.dropped_frames,
    )
}

/// Writes `value` under `key`, applies locally, appends to WAL, awaits gossip capacity.
///
/// Returns `false` without applying anything if `key.len() + value.len()` exceeds
/// [`MAX_KV_WRITE_BYTES`](crate::framing::MAX_KV_WRITE_BYTES) — see [`kv_set`].
pub async fn kv_set_async(ctx: &CoreCtx, key: Arc<str>, value: Bytes) -> bool {
    if reject_oversized_write(&key, value.len()) {
        return false;
    }
    let update = make_gossip_update(&ctx.node_id, ctx.default_ttl, key, value, false, &ctx.hlc);
    // Apply first, then persist (persistence.rs durability invariant 1): awaiting the
    // Flush-mode ack *before* applying left a window in which the writer's threshold
    // snapshot scanned a store without this write and then truncated its WAL record.
    apply_and_notify(&ctx.kv_state, &update);
    if let Some(wal) = ctx.wal.get()
        && let Err(e) = wal.append(sync_entry_from(&update)).await
    {
        // The public `set_async`/`delete_async` return the gossip-queue bool, not durability;
        // surfacing per-write durability is the contracts plan's PR 2/3. Until then the
        // failure must at least be legible (Run 61 finding: it was silently discarded).
        tracing::warn!(key = %update.key, error = %e, "kv write applied and gossiped but its WAL append failed");
    }
    #[cfg(feature = "metrics")]
    metrics::counter!("gossip_kv_writes_total").increment(1);
    let tls = ctx.tls.get().map(Arc::as_ref);
    let msg = make_kv_wire_msg(update, ctx.node_id.id_hash(), tls);
    dispatch_gossip_send(
        &ctx.gossip_txs, msg,
        ctx.node_id.id_hash(), ForwardHint::All,
    ).await
}

/// Write `value` under `key` **and return what it established** — the contracts axis' write path
/// (item 1 PR 2; `docs/design/contracts-receipts.md`).
///
/// Unlike [`kv_set_async`], whose `bool` is the gossip-queue result, this reports the receipt:
/// the local-application rung (`Applied`/`Superseded` under LWW) and the local-sync rung
/// (`OnDisk` / `Buffered` / `Failed` / `NotConfigured`). It does **not** claim any rung above:
/// nothing here establishes that a peer holds the operation.
///
/// `stamp` re-submits a **pre-stamped** update: a retry must reuse its first attempt's HLC rather
/// than tick a fresh one (D11), or the same operation would rank differently under LWW on every
/// attempt. `None` stamps a new one.
pub async fn kv_set_with_receipt(
    ctx: &CoreCtx,
    op: crate::receipt::OperationId,
    attempt: crate::receipt::AttemptId,
    key: Arc<str>,
    value: Bytes,
    stamp: Option<u64>,
) -> Result<crate::receipt::WriteReceipt, crate::receipt::ReceiptError> {
    use crate::receipt::{content_hash, LocalDurability, ReceiptError, WriteReceipt};

    if reject_oversized_write(&key, value.len()) {
        return Err(ReceiptError::Rejected(format!(
            "key + value exceeds MAX_KV_WRITE_BYTES ({} bytes)",
            crate::framing::MAX_KV_WRITE_BYTES
        )));
    }

    let hash = content_hash(&key, &value, false);
    let update = match stamp {
        Some(ts) => make_gossip_update_stamped(&ctx.node_id, ctx.default_ttl, Arc::clone(&key), value, false, ts),
        None => make_gossip_update(&ctx.node_id, ctx.default_ttl, Arc::clone(&key), value, false, &ctx.hlc),
    };
    let ts = update.timestamp;

    // Apply first, then persist (persistence.rs durability invariant 1).
    let application = crate::store::apply_and_notify_reporting(&ctx.kv_state, &update);

    // `append_acked`, never `append`: in `Async`/`Os` the latter is a `try_send` that returns `Ok`
    // even when the queue is full or the writer is gone, so a receipt built on it would claim the
    // bytes reached the operating system when they may never have left this process (review,
    // 2026-09-15). The acknowledgement is of the *write*; whether it was also synced depends on the
    // mode, which is what separates `OnDisk` from `Buffered`.
    let local_durability = match ctx.wal.get() {
        None => LocalDurability::NotConfigured,
        Some(wal) => match wal.append_acked(sync_entry_from(&update)).await {
            Ok(()) => match ctx.config.persistence.as_ref().map(|p| p.sync_mode) {
                Some(crate::config::SyncMode::Flush) => LocalDurability::OnDisk,
                _ => LocalDurability::Buffered,
            },
            Err(e) => LocalDurability::Failed(e.to_string()),
        },
    };

    #[cfg(feature = "metrics")]
    metrics::counter!("gossip_kv_writes_total").increment(1);
    // The rung actually reached, by its own stable tag. Without this the durability *distribution*
    // is invisible in aggregate: a receipt reports per response, and an operator running `Async`
    // has no way to see how much of their traffic is settling at `buffered` rather than `on_disk`.
    // `tag()` is the vocabulary the gateway JSON and both SDKs already share, so there is one set
    // of names, and it is a closed set of four.
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_kv_receipts_total", "local_durability" => local_durability.tag())
        .increment(1);
    let tls = ctx.tls.get().map(Arc::as_ref);
    let msg = make_kv_wire_msg(update, ctx.node_id.id_hash(), tls);
    let queued = dispatch_gossip_send(&ctx.gossip_txs, msg, ctx.node_id.id_hash(), ForwardHint::All).await;

    Ok(WriteReceipt::new(op, attempt, key, hash, ts, application)
        .with_local_durability(local_durability)
        .queued(queued))
}

/// Write `value` under `key` **only if it can be made durable first** — the contracts axis' strong
/// path (item 1 PR 3; `docs/design/contracts-receipts.md` §2.1, §2.2).
///
/// The ordering is deliberately the reverse of every other write site: **persist → apply → gossip**.
/// `append_sync` forces an `fdatasync` in every [`SyncMode`](crate::config::SyncMode), and only when
/// it returns does the value become visible locally or reach a peer. A caller that receives
/// [`ReceiptError::DurabilityNotEstablished`] therefore knows something the ordinary path can never
/// tell it: **nothing became visible at this node** — no reader saw it, no subscriber fired, nothing
/// gossiped.
///
/// Persist-first is admissible here *only* because the snapshot merges the WAL tail before
/// truncating (D8): a record fsynced before the caller applied it cannot be discarded. That
/// dependency is pinned by `regression_snapshot_retains_wal_record_acked_before_local_apply` and
/// `regression_writer_threshold_snapshot_right_after_ack_keeps_write`; if either fails, this
/// ordering is no longer admissible.
///
/// A node with **no persistence configured** is refused outright rather than answered with a
/// receipt that claims nothing: the caller asked for durability as a contract, and a node that
/// cannot provide it must say so (posture rule 3(i) — prevention requested by the caller).
pub async fn kv_set_requiring_sync(
    ctx: &CoreCtx,
    op: crate::receipt::OperationId,
    attempt: crate::receipt::AttemptId,
    key: Arc<str>,
    value: Bytes,
    stamp: Option<u64>,
) -> Result<crate::receipt::WriteReceipt, crate::receipt::ReceiptError> {
    use crate::receipt::{content_hash, LocalDurability, ReceiptError, WriteReceipt};

    if reject_oversized_write(&key, value.len()) {
        return Err(ReceiptError::Rejected(format!(
            "key + value exceeds MAX_KV_WRITE_BYTES ({} bytes)",
            crate::framing::MAX_KV_WRITE_BYTES
        )));
    }

    let Some(wal) = ctx.wal.get() else {
        return Err(ReceiptError::DurabilityNotEstablished {
            persistence_configured: false,
            reason: "no WAL is attached to this node".to_string(),
        });
    };

    let hash = content_hash(&key, &value, false);
    let update = match stamp {
        Some(ts) => make_gossip_update_stamped(&ctx.node_id, ctx.default_ttl, Arc::clone(&key), value, false, ts),
        None => make_gossip_update(&ctx.node_id, ctx.default_ttl, Arc::clone(&key), value, false, &ctx.hlc),
    };
    let ts = update.timestamp;

    // 1. Durable first. Nothing is visible yet: no store entry, no notification, no frame.
    if let Err(e) = wal.append_sync(sync_entry_from(&update)).await {
        return Err(ReceiptError::DurabilityNotEstablished {
            persistence_configured: true,
            reason: e.to_string(),
        });
    }

    // 2. Now apply, and 3. gossip. The WAL-tail merge is what makes this ordering safe.
    let application = crate::store::apply_and_notify_reporting(&ctx.kv_state, &update);

    #[cfg(feature = "metrics")]
    metrics::counter!("gossip_kv_writes_total").increment(1);
    // Always `on_disk` here — this path returns a receipt only after the sync returned. The two
    // refusals above never reach it, which is the point: a required-sync write that did not
    // establish durability applied nothing, so there is no receipt to count.
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_kv_receipts_total", "local_durability" => LocalDurability::OnDisk.tag())
        .increment(1);
    let tls = ctx.tls.get().map(Arc::as_ref);
    let msg = make_kv_wire_msg(update, ctx.node_id.id_hash(), tls);
    let queued = dispatch_gossip_send(&ctx.gossip_txs, msg, ctx.node_id.id_hash(), ForwardHint::All).await;

    Ok(WriteReceipt::new(op, attempt, key, hash, ts, application)
        .with_local_durability(LocalDurability::OnDisk)
        .queued(queued))
}

/// Tombstones `key`, applies locally, queues WAL (try-send), gossips (try-send).
pub fn kv_delete(ctx: &CoreCtx, key: Arc<str>) -> bool {
    let update = make_gossip_update(&ctx.node_id, ctx.default_ttl, key, Bytes::new(), true, &ctx.hlc);
    // Apply first, then persist: the store is never behind the WAL, so a writer-side
    // snapshot scan always contains every record the writer has been handed
    // (persistence.rs durability invariant 1).
    apply_and_notify(&ctx.kv_state, &update);
    if let Some(wal) = ctx.wal.get() {
        wal.append_try(sync_entry_from(&update));
    }
    #[cfg(feature = "metrics")]
    metrics::counter!("gossip_kv_deletes_total").increment(1);
    let tls = ctx.tls.get().map(Arc::as_ref);
    let msg = make_kv_wire_msg(update, ctx.node_id.id_hash(), tls);
    dispatch_gossip_try_send(
        &ctx.gossip_txs, msg,
        ctx.node_id.id_hash(), ForwardHint::All, &ctx.kv_state.dropped_frames,
    )
}

/// Tombstones `key`, applies locally, appends to WAL, awaits gossip capacity.
pub async fn kv_delete_async(ctx: &CoreCtx, key: Arc<str>) -> bool {
    let update = make_gossip_update(&ctx.node_id, ctx.default_ttl, key, Bytes::new(), true, &ctx.hlc);
    // Apply first, then persist (persistence.rs durability invariant 1): awaiting the
    // Flush-mode ack *before* applying left a window in which the writer's threshold
    // snapshot scanned a store without this write and then truncated its WAL record.
    apply_and_notify(&ctx.kv_state, &update);
    if let Some(wal) = ctx.wal.get()
        && let Err(e) = wal.append(sync_entry_from(&update)).await
    {
        // The public `set_async`/`delete_async` return the gossip-queue bool, not durability;
        // surfacing per-write durability is the contracts plan's PR 2/3. Until then the
        // failure must at least be legible (Run 61 finding: it was silently discarded).
        tracing::warn!(key = %update.key, error = %e, "kv write applied and gossiped but its WAL append failed");
    }
    #[cfg(feature = "metrics")]
    metrics::counter!("gossip_kv_deletes_total").increment(1);
    let tls = ctx.tls.get().map(Arc::as_ref);
    let msg = make_kv_wire_msg(update, ctx.node_id.id_hash(), tls);
    dispatch_gossip_send(
        &ctx.gossip_txs, msg,
        ctx.node_id.id_hash(), ForwardHint::All,
    ).await
}

/// Returns live members of `group` from Layer I KV (`grp/{group}/`).
pub fn group_members_ctx(ctx: &CoreCtx, group: &str) -> Vec<crate::node_id::NodeId> {
    let prefix = crate::signal::grp_prefix(group);
    kv_scan_prefix(ctx, &prefix)
        .into_iter()
        .filter_map(|(key, _)| {
            key.strip_prefix(&prefix)
                .and_then(|s| s.parse::<crate::node_id::NodeId>().ok())
        })
        .collect()
}

#[cfg(test)]
mod delivery_shed_tests {
    use super::*;
    use crate::node_id::NodeId;
    use crate::signal::signal_kind;
    use std::time::Duration;

    fn sig(kind: &str, scope: SignalScope, sender: NodeId) -> Signal {
        Signal { kind: Arc::from(kind), scope, payload: Bytes::new(), sender, nonce: 1 }
    }

    /// Regression for the opacity-governor flake (`test_manage_opacity_gate_vetoes_then_library_overrides`,
    /// flaky Runs 27–36): the governor's own `BOUNDARY_OPAQUE` announcement is `System`-scoped and was
    /// subject to the probabilistic local shed in `deliver_locally`. Under gossip backpressure
    /// (`combined_fill > 0`) the single emission could be dropped from *local* delivery — the "I'm now
    /// shedding" signal shed by the shedding mechanism. At `combined_fill == 1.0` the shed is
    /// deterministic (`fastrand::f32()` is in `[0,1)`, never `>= 1.0`), so this pins the fix.
    #[test]
    fn boundary_transition_signals_are_never_locally_shed() {
        let node = NodeId::new("127.0.0.1", 9000).unwrap();
        let boundary = RwLock::new(Boundary::new(node.clone()));
        let handlers = SignalHandlers::new(Duration::from_secs(60));

        let mut opaque_rx = handlers.register_with_capacity(Arc::from(signal_kind::BOUNDARY_OPAQUE), 4);
        let mut clear_rx  = handlers.register_with_capacity(Arc::from(signal_kind::BOUNDARY_TRANSPARENT), 4);
        let mut work_rx   = handlers.register_with_capacity(Arc::from("work"), 4);

        // Sanity: an ordinary Cluster-scoped signal IS shed deterministically at fill 1.0.
        deliver_locally(&boundary, &handlers, &sig("work", SignalScope::Cluster, node.clone()), 1.0);
        assert!(work_rx.try_recv().is_err(), "ordinary Cluster signal is shed at fill 1.0");

        // The fix: boundary-transition control signals must still be delivered locally at fill 1.0.
        deliver_locally(&boundary, &handlers, &sig(signal_kind::BOUNDARY_OPAQUE, SignalScope::Cluster, node.clone()), 1.0);
        assert!(opaque_rx.try_recv().is_ok(),
            "BOUNDARY_OPAQUE must be delivered locally even at fill 1.0 (control signals are not load-shed)");

        deliver_locally(&boundary, &handlers, &sig(signal_kind::BOUNDARY_TRANSPARENT, SignalScope::Cluster, node.clone()), 1.0);
        assert!(clear_rx.try_recv().is_ok(),
            "BOUNDARY_TRANSPARENT must be delivered locally even at fill 1.0");
    }
}
