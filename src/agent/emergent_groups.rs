//! Emergent group formation (Phase 3g/3h) plus the Phase-7 group-requirement
//! opacity coupling.
//!
//! `define_capability_group` publishes a `CapabilityGroupDef` under
//! `cap-group/{group}`; the agent-wide `watch_capability_group_definitions`
//! background task subscribes to `cap-group/`, `cap/{self}/`, this node's own
//! `grp/{group}/{self}` entries and the membership intents, and reconciles
//! whether this node should self-join each emergent group based on whether its
//! own capabilities match the group's filter, and which groups it projects.
//!
//! When the node projects a group, the watcher spawns one consolidated
//! `run_group_membership_task` per group that internally:
//! - re-asserts `gcap/{group}/{ns}/{name}/{self}` for every `provides`
//!   capability on a `GCAP_REASSERT_INTERVAL` ticker — the group-level
//!   projection that inter-group wiring (see `super::wiring`) discovers
//!   and routes to;
//! - tracks `sys/load/{self}/group-req/{group}/{idx}` opacity for every
//!   `requires` filter, writing when unsatisfied and tombstoning when
//!   satisfied, composing with load-based opacity through the existing
//!   `is_self_opaque` scanner.
//!
//! Pre-C3 each provide + each requirement was its own spawned tokio task.
//! After C3 it is one task per group, regardless of provides/requires count.
//!
//! **Governed groups (WS-C elastic sizing; ER9).** A group under a live
//! `sys/govern/membership/{group}` intent, and every electorate group, is *governed*
//! (`membership_governor::governed_groups`, the one definition). Its **membership** is not this
//! watcher's: it neither joins the group on a cap match nor leaves it on a mismatch, so the
//! [`MembershipGovernor`](super::membership_governor)'s `[min, max]` / `drain` can hold against the
//! otherwise-unconditional cap-match auto-join (#56). Its **projection** is: the watcher runs the
//! group's membership task exactly while `grp/{group}/{self}` is live in the store and the
//! definition's filter matches one of this node's own capabilities — whoever wrote the membership
//! (the governor, `/gateway/govern/group`, `join_group`, a peer) — and drops it when either stops,
//! which tombstones the `gcap/` keys and clears the opacity keys. A membership is never stated as a
//! capability the node does not hold (D9a). When an intent evaporates the group reverts to emergent
//! membership, and a matching node that is not in `grp/` is written back into it once (`was_governed`).
//!
//! **A projection never outlives the membership**, emergent groups included: an emergent group
//! this node is seen to leave while projecting it (`leave_group`, `DELETE /gateway/govern/group`,
//! a peer's tombstone) stops being projected and is not rejoined (`left`) until the node is a
//! member again or the filter or the definition goes, or the node restarts (the set is not
//! persisted, so a restart rejoins a group left by hand, as before ER9 — and heals a node a
//! forged tombstone kept out).

use crate::capability::{CapEntry, Capability, CapFilter, CapabilityGroupDef, WiringStatus};
use crate::node_id::NodeId;
use crate::signal::{LoadState, encode_load_state};
use ahash::{AHashMap, AHashSet};
use bytes::Bytes;
use std::{sync::Arc, time::Duration};
use tokio::{sync::{oneshot, watch}, time};
use tracing::warn;

use super::TaskCtx;
use super::capability_ops::{
    await_shutdown, now_ms, scan_prefix_kv,
    subscribe_prefix_on_kv, WATCHER_DEBOUNCE_WINDOW,
};
use super::wiring::wiring_snapshot;

/// Interval at which group-level `gcap/` projections are re-asserted by each
/// cap-joined member. Conservative default chosen to amortise gossip cost;
/// the projection KV entry itself only expires on tombstone, so a slow
/// re-advertise still keeps late joiners in sync via anti-entropy.
const GCAP_REASSERT_INTERVAL: Duration = Duration::from_secs(60);

/// Per-group bookkeeping for the emergent-group watcher: every cap-joined
/// group has one consolidated `run_group_membership_task` (C3) that owns
/// both the `gcap/` reassertion loop and per-requirement opacity tracking.
/// Dropping the projection closes `_cancel`, which the membership task
/// observes and exits gracefully — tombstoning every `gcap/` and
/// `sys/load/{self}/group-req/{group}/{idx}` key it owns.
struct GroupProjection {
    /// Snapshot of the def's `provides` used to detect changes during
    /// reconciliation. When this differs from a new def, the projection
    /// is rebuilt (cancel old, spawn new).
    provides: Vec<Capability>,
    /// Snapshot of the def's `requires`, used the same way.
    requires: Vec<CapFilter>,
    /// Cancel sender for the consolidated membership task. Drop or
    /// `send(())` to make the task tombstone everything it wrote and
    /// return.
    #[allow(dead_code)]
    _cancel:  oneshot::Sender<()>,
}

/// The watcher's wake sources (ER9 item 5). All four feed the one debounce loop; none is a timer — every
/// refresh of an own advertisement already wakes the watcher, which is what notices an evaporated intent.
pub(crate) struct WatcherWakes {
    /// `cap-group/` — a definition written, changed or tombstoned.
    pub(crate) defs:    watch::Receiver<u64>,
    /// `cap/{self}/` — this node's own capabilities.
    pub(crate) own:     watch::Receiver<u64>,
    /// `grp/` keys ending in `/{self}` — this node's memberships, whoever wrote them: a governed group's
    /// projection follows them.
    pub(crate) grp:     watch::Receiver<u64>,
    /// `sys/govern/membership/` — an intent published, changed or tombstoned: a group becomes governed or not.
    pub(crate) intents: watch::Receiver<u64>,
}

/// Background task: watches `cap-group/` for emergent group definitions and keeps this node's
/// `grp/` membership in sync with whether its own capabilities match each def's filter — for
/// emergent groups — and its `gcap/` projections in sync with its membership — for governed ones.
/// See the module doc.
pub(crate) async fn watch_capability_group_definitions(
    ctx:             Arc<TaskCtx>,
    own_node_id:     NodeId,
    wakes:           WatcherWakes,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let WatcherWakes { defs: mut def_rx, own: mut own_rx, grp: mut grp_rx, intents: mut intent_rx } = wakes;
    // Tracks the groups we are projecting — `gcap/` and opacity — emergent and governed alike.
    // Dropping a `GroupProjection` closes its cancel sender, which the membership task observes
    // and exits gracefully via a tombstone.
    let mut joined: AHashMap<Arc<str>, GroupProjection> = AHashMap::new();
    // The groups governed for this node at the last pass, task-local: what tells an intent's
    // evaporation (governed → emergent) apart from an emergent group the application left by hand.
    let mut was_governed: AHashSet<Arc<str>> = AHashSet::new();
    // The emergent groups this node was seen to leave while projecting them (by hand, through a
    // route, or by a peer's tombstone), task-local: not projected and not rejoined until the node
    // is a member again, or the filter or the definition goes.
    let mut left: AHashSet<Arc<str>> = AHashSet::new();

    // Initial reconciliation in case definitions or own caps already exist.
    reconcile_emergent_groups(&ctx, &own_node_id, &shutdown_rx, &mut joined, &mut was_governed, &mut left);

    'outer: loop {
        tokio::select! { biased;
            _ = await_shutdown(&mut shutdown_rx) => break 'outer,
            r = def_rx.changed()    => { if r.is_err() { break 'outer; } }
            r = own_rx.changed()    => { if r.is_err() { break 'outer; } }
            r = grp_rx.changed()    => { if r.is_err() { break 'outer; } }
            r = intent_rx.changed() => { if r.is_err() { break 'outer; } }
        }
        // Coalesce burst writes (anti-entropy sync, bulk cap-group push)
        // into a single reconcile pass. See WATCHER_DEBOUNCE_WINDOW.
        let deadline = time::Instant::now() + WATCHER_DEBOUNCE_WINDOW;
        loop {
            tokio::select! { biased;
                _ = await_shutdown(&mut shutdown_rx) => break 'outer,
                _ = time::sleep_until(deadline) => break,
                r = def_rx.changed()    => { if r.is_err() { break 'outer; } }
                r = own_rx.changed()    => { if r.is_err() { break 'outer; } }
                r = grp_rx.changed()    => { if r.is_err() { break 'outer; } }
                r = intent_rx.changed() => { if r.is_err() { break 'outer; } }
            }
        }
        reconcile_emergent_groups(&ctx, &own_node_id, &shutdown_rx, &mut joined, &mut was_governed, &mut left);
    }

    // Best-effort: tombstone the emergent memberships we hold so peers see us leave
    // the emergent groups cleanly. A governed group's membership is not ours to write
    // (the governor's, a route's or the application's). "Governed" is read again here,
    // not taken from the last pass, which can be a debounce window or an evaporation old;
    // the wall reading goes through the seam and is taken only when there is a projection
    // to decide on, so a recording with no group holds the reads it held before. Dropping
    // `joined` also fires the membership tasks' cancel senders, which tombstone the
    // `gcap/` entries.
    let governed_now = if joined.is_empty() {
        AHashSet::new()
    } else {
        super::membership_governor::governed_groups(
            &ctx, now_ms(), super::membership_governor::GovernedFor::Node(&own_node_id),
        )
    };
    let exit_groups: Vec<Arc<str>> = joined.keys().filter(|g| !governed_now.contains(g.as_ref())).cloned().collect();
    for g in exit_groups {
        emit_membership(&ctx, &own_node_id, &g, true);
    }
    drop(joined);
}

/// Whether `grp/{group}/{own}` is live in the store — the LWW truth, read as the connection's
/// boundary sync reads it (`mycelium-core/src/connection.rs`, `parse_own_grp_key`).
fn is_live_member(ctx: &TaskCtx, own: &NodeId, group: &str) -> bool {
    let key = crate::signal::grp_member_key(group, own);
    ctx.kv_state.store.pin().get(key.as_str()).is_some_and(|e| e.data.is_some())
}

/// This node's own capabilities, for the group projection: those it has not stopped refreshing. A node
/// restarted over persisted `cap/` entries it no longer advertises would otherwise join the groups they match and
/// project a fresh `gcap/` that demand and wiring count (#734's review, finding 3).
fn own_capabilities(kv_state: &crate::store::KvState, own: &NodeId, now_ms: u64) -> Vec<Capability> {
    super::capability_ops::own_live_caps_at(kv_state, own, now_ms)
}

/// One reconciliation pass: snapshot own caps + every `cap-group/` def, decide which groups we
/// should project, and update `gcap/{group}/{ns}/{name}/{self}` projections accordingly — and,
/// for **emergent** groups only, the `grp/{group}/{self}` membership too. "Am I a member" is split
/// from "do I project" (ER9): an emergent group is joined and projected on a filter match; a
/// governed group is projected while this node is a live member of it and matches the filter,
/// and its membership is left to whoever governs it.
fn reconcile_emergent_groups(
    ctx:          &Arc<TaskCtx>,
    own:          &NodeId,
    shutdown_rx:  &watch::Receiver<bool>,
    joined:       &mut AHashMap<Arc<str>, GroupProjection>,
    was_governed: &mut AHashSet<Arc<str>>,
    left:         &mut AHashSet<Arc<str>>,
) {
    // Own capabilities snapshot (used both for filter matching and for the
    // provides decision; we project everything the def specifies, regardless
    // of whether we hold the same direct capability ourselves — the def is
    // the group's collective assertion, not a per-member echo of its own caps).
    // Current cap-group definitions, keyed by group name.
    let mut defs: AHashMap<Arc<str>, CapabilityGroupDef> = AHashMap::new();
    for (key, bytes) in scan_prefix_kv(&ctx.kv_state, "cap-group/") {
        let Some(group_name) = key.strip_prefix("cap-group/") else { continue };
        let Some(def) = CapabilityGroupDef::decode(&bytes) else {
            warn!(key = %key, "malformed CapabilityGroupDef under cap-group/ — peer sent bytes that did not decode");
            continue;
        };
        // A definition whose group name or `provides` name is not a key segment would project
        // `gcap/{group}/{ns}/{name}/{self}` and `grp/{group}/{self}` keys no reader parses. No upgraded door
        // writes one, but a 2.33 peer or state persisted before the upgrade can (#629's review, finding 1).
        let problem = crate::capability::capability_group_name_problem(group_name)
            .or_else(|| def.provides.iter().find_map(|c| crate::capability::capability_name_problem(&c.namespace, &c.name)));
        if let Some(problem) = problem {
            warn!(key = %key, problem, "CapabilityGroupDef skipped: a name in it is not a key segment");
            continue;
        }
        defs.insert(Arc::from(group_name), def);
    }

    // One wall reading serves the pass — the own entries' staleness here and the intents' below —
    // taken where the pass always took it, so a recording holds the reads it held before own
    // entries were judged (#734's confirm round: a second read in this event-driven pass diverged a replay).
    let now = now_ms();
    let own_caps = own_capabilities(&ctx.kv_state, own, now);

    // The groups whose membership is not this watcher's — the one definition, read for this node
    // (an intent `target`ed at another node does not govern it here, as the governor reads it).
    let governed = super::membership_governor::governed_groups(
        ctx, now, super::membership_governor::GovernedFor::Node(own),
    );

    // want_projected: every group whose filter one of our own capabilities matches, **while this
    // node is a member of it** — a projection is the node's statement that it is a member, so it
    // never outlives the membership (ER9 review). Governed groups: only while `grp/{g}/{self}` is
    // live (D9a: a join made for the node by an intent, a route or a peer does not make it state a
    // capability it does not hold, and a member that stops matching stops projecting). Emergent
    // groups: this watcher writes the membership when it first projects the group (`to_join`);
    // once projected, a group seen not live — left by hand, through `DELETE /gateway/govern/group`,
    // or tombstoned by a peer — drops its projection and goes into `left`, and is not rejoined
    // until the node is a member again or the filter or the definition goes. The one exception is
    // an evaporation: a group that has just stopped being governed, and this node is not in its
    // roster (the governor drained it), is written back into it once (item 4) — unless the node
    // had left it itself.
    left.retain(|g| defs.get(g).is_some_and(|d| own_caps.iter().any(|c| d.filter.matches(c))));
    let mut want_projected: AHashSet<Arc<str>> = AHashSet::new();
    let mut to_join: Vec<Arc<str>> = Vec::new();
    for (group, def) in &defs {
        if !own_caps.iter().any(|c| def.filter.matches(c)) { continue; }
        let member = is_live_member(ctx, own, group);
        if member { left.remove(group.as_ref()); }
        if governed.contains(group.as_ref()) {
            if member { want_projected.insert(Arc::clone(group)); }
            continue;
        }
        if left.contains(group.as_ref()) { continue; }
        let evaporated = was_governed.contains(group.as_ref()) && !member;
        if !member && joined.contains_key(group.as_ref()) && !evaporated {
            // Seen to leave: stop projecting, do not rejoin — unless it is a drain. `governed` was
            // read at the top of the pass and the membership here, so an intent that arrived in
            // between, and a governor that drained this node on it, would read as a hand leave and
            // later lose the evaporation rejoin. The intents are read again for this group before it
            // goes into `left`, under the pass's one wall reading (an intent that new is fresh by it;
            // no second read, so a recording holds the reads it held before). Governed now: the
            // projection is dropped and the next pass treats the group as governed. Not tested: the
            // window lies between two reads of one pass and no test can place a write inside it
            // deterministically.
            if !super::membership_governor::is_governed(
                ctx, group, now, super::membership_governor::GovernedFor::Node(own),
            ) {
                left.insert(Arc::clone(group));
            }
            continue;
        }
        if !member { to_join.push(Arc::clone(group)); }
        want_projected.insert(Arc::clone(group));
    }
    for group in &to_join {
        emit_membership(ctx, own, group, false);
    }

    // Project newly-wanted groups; spawn one consolidated membership task per
    // group (provides + requires handled internally).
    for group in &want_projected {
        let def      = defs.get(group).expect("present, scanned above");
        let provides = def.provides.clone();
        let requires = def.requires.clone();

        if !joined.contains_key(group.as_ref()) {
            let _cancel = spawn_group_membership(
                ctx, own, group, &provides, &requires, shutdown_rx,
            );
            joined.insert(Arc::clone(group), GroupProjection {
                provides, requires, _cancel,
            });
            continue;
        }
        // Already projecting — rebuild the membership task only when the def
        // actually changed.
        let (current_provides, current_requires) = {
            let current = joined.get(group).expect("just checked");
            (current.provides.clone(), current.requires.clone())
        };
        if current_provides == provides && current_requires == requires { continue; }
        // Drop old projection (cancels old membership task → tombstones
        // its keys) then spawn a fresh one with the new def.
        let _ = joined.remove(group.as_ref());
        let _cancel = spawn_group_membership(
            ctx, own, group, &provides, &requires, shutdown_rx,
        );
        joined.insert(Arc::clone(group), GroupProjection {
            provides, requires, _cancel,
        });
    }

    // Stop projecting groups we no longer want — the def disappeared, our caps stopped matching,
    // or this node is no longer a member. Dropping the `GroupProjection` tombstones the gcap
    // entries and clears the opacity keys: for a governed group, that is the tombstone on a
    // drain. Only an emergent group's membership is tombstoned here
    // (`emit_membership(leaving = true)`), and not one in `left` (already not a member); a
    // governed group's is the governor's call (drain / over-max), a route's or the
    // application's, never this watcher's.
    let to_drop: Vec<Arc<str>> = joined.keys()
        .filter(|g| !want_projected.contains(g.as_ref()))
        .cloned()
        .collect();
    for g in to_drop {
        if !governed.contains(g.as_ref()) && !left.contains(g.as_ref()) {
            emit_membership(ctx, own, &g, true);
        }
        joined.remove(&g);
    }
    *was_governed = governed;
}

/// Spawns the single consolidated membership task for `group` and returns its
/// cancel sender. The task owns both the `gcap/` reassertion loop for every
/// `provides` capability AND opacity tracking for every `requires` filter.
/// Pre-C3 this was `provides.len() + requires.len()` separate tokio tasks
/// per group; after C3 it is one.
fn spawn_group_membership(
    ctx:         &Arc<TaskCtx>,
    own:         &NodeId,
    group:       &Arc<str>,
    provides:    &[Capability],
    requires:    &[CapFilter],
    shutdown_rx: &watch::Receiver<bool>,
) -> oneshot::Sender<()> {
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    tokio::spawn(run_group_membership_task(
        Arc::clone(ctx),
        own.clone(),
        Arc::clone(group),
        provides.to_vec(),
        requires.to_vec(),
        cancel_rx,
        shutdown_rx.clone(),
    ));
    cancel_tx
}

/// Consolidated per-group membership task. Owns:
/// - the reassertion ticker that re-writes every `gcap/{group}/{ns}/{name}/{self}`
///   on `GCAP_REASSERT_INTERVAL`;
/// - the opacity tracker that writes/clears `sys/load/{self}/group-req/{group}/{idx}`
///   for every requirement filter as wiring status changes.
///
/// Wakes on cancel, shutdown, the reassert ticker, or any `cap/`/`gcap/`
/// change (debounced by [`WATCHER_DEBOUNCE_WINDOW`]). On exit, tombstones
/// every `gcap/` and any opacity key it had written.
async fn run_group_membership_task(
    ctx:             Arc<TaskCtx>,
    own:             NodeId,
    group:           Arc<str>,
    provides:        Vec<Capability>,
    requires:        Vec<CapFilter>,
    mut cancel_rx:   oneshot::Receiver<()>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    use crate::framing::{
        dispatch_gossip_send, dispatch_gossip_try_send, ForwardHint,
        make_gossip_update,
    };
    use crate::store::apply_and_notify;

    let gcap_keys: Vec<Arc<str>> = provides.iter().map(|c| {
        Arc::from(format!("gcap/{}/{}/{}/{}", group, c.namespace, c.name, own).as_str())
    }).collect();
    let opacity_keys: Vec<Arc<str>> = (0..requires.len()).map(|i| {
        Arc::from(format!("sys/load/{}/group-req/{}/{}", own, group, i).as_str())
    }).collect();
    let mut opaque_written = vec![false; requires.len()];

    let write_provide = |key: &Arc<str>, cap: &Capability| {
        let payload = CapEntry {
            capability:          cap.clone(),
            refresh_interval_ms: GCAP_REASSERT_INTERVAL.as_millis() as u64,
        }.encode();
        // Through the soft-state gate: closed by shutdown before its sweep (#627's review).
        ctx.soft_state_gate.write(|| {
            let upd = make_gossip_update(
                &ctx.node_id, ctx.default_ttl, Arc::clone(key), payload, false, &ctx.hlc,
            );
            apply_and_notify(&ctx.kv_state, &upd);
            dispatch_gossip_try_send(
                &ctx.gossip_txs, crate::framing::local_kv_wire_msg(&ctx, upd),
                ctx.node_id.id_hash(), ForwardHint::All, &ctx.kv_state.dropped_frames,
            );
        });
    };
    let write_opaque = |key: &Arc<str>| {
        let payload = encode_load_state(&LoadState {
            fill_ratio:    1.0,
            is_opaque:     true,
            written_at_ms: now_ms(),
        });
        // Through the soft-state gate: closed by shutdown before its sweep (#627's review).
        ctx.soft_state_gate.write(|| {
            let upd = make_gossip_update(
                &ctx.node_id, ctx.default_ttl, Arc::clone(key), payload, false, &ctx.hlc,
            );
            apply_and_notify(&ctx.kv_state, &upd);
            dispatch_gossip_try_send(
                &ctx.gossip_txs, crate::framing::local_kv_wire_msg(&ctx, upd),
                ctx.node_id.id_hash(), ForwardHint::All, &ctx.kv_state.dropped_frames,
            );
        });
    };
    let clear_opaque = |key: &Arc<str>| {
        let upd = make_gossip_update(
            &ctx.node_id, ctx.default_ttl, Arc::clone(key), Bytes::new(), true, &ctx.hlc,
        );
        apply_and_notify(&ctx.kv_state, &upd);
        dispatch_gossip_try_send(
            &ctx.gossip_txs, crate::framing::local_kv_wire_msg(&ctx, upd),
            ctx.node_id.id_hash(), ForwardHint::All, &ctx.kv_state.dropped_frames,
        );
    };
    let is_satisfied = |filter: &CapFilter| -> bool {
        !matches!(wiring_snapshot(&ctx.kv_state, filter), WiringStatus::Unwired { .. })
    };

    // Initial writes: project all provides and evaluate every requirement.
    for (key, cap) in gcap_keys.iter().zip(&provides) {
        write_provide(key, cap);
    }
    for (i, filter) in requires.iter().enumerate() {
        if !is_satisfied(filter) {
            write_opaque(&opacity_keys[i]);
            opaque_written[i] = true;
        }
    }

    let mut cap_rx  = subscribe_prefix_on_kv(&ctx.kv_state, Arc::<str>::from("cap/"));
    let mut gcap_rx = subscribe_prefix_on_kv(&ctx.kv_state, Arc::<str>::from("gcap/"));

    // Through the timer seam (item 6).
    let mut ticker = mycelium_core::sim_seam::interval_ms(
        "gcap/reassert",
        GCAP_REASSERT_INTERVAL.as_millis() as u64,
        time::MissedTickBehavior::Skip,
    );
    ticker.tick().await; // Consume the immediate first tick — we just wrote.

    'main: loop {
        tokio::select! { biased;
            _ = &mut cancel_rx                   => break 'main,
            _ = await_shutdown(&mut shutdown_rx) => break 'main,
            _ = ticker.tick() => {
                for (key, cap) in gcap_keys.iter().zip(&provides) {
                    write_provide(key, cap);
                }
                continue 'main;
            }
            r = cap_rx.changed()  => { if r.is_err() { break 'main; } }
            r = gcap_rx.changed() => { if r.is_err() { break 'main; } }
        }
        // Coalesce: drain further cap/gcap fires for the debounce window
        // before recomputing every requirement's satisfaction once.
        let deadline = time::Instant::now() + WATCHER_DEBOUNCE_WINDOW;
        loop {
            tokio::select! { biased;
                _ = time::sleep_until(deadline) => break,
                r = cap_rx.changed()  => { if r.is_err() { break 'main; } }
                r = gcap_rx.changed() => { if r.is_err() { break 'main; } }
            }
        }
        for (i, filter) in requires.iter().enumerate() {
            let satisfied = is_satisfied(filter);
            match (opaque_written[i], satisfied) {
                (false, false) => { write_opaque(&opacity_keys[i]); opaque_written[i] = true; }
                (true,  true)  => { clear_opaque(&opacity_keys[i]); opaque_written[i] = false; }
                _ => {}
            }
        }
    }

    // Tombstone every gcap projection (use ordered send so the final
    // tombstones actually leave the agent before the task is dropped),
    // and clear any still-opaque requirement entries.
    for key in &gcap_keys {
        let upd = make_gossip_update(
            &ctx.node_id, ctx.default_ttl, Arc::clone(key), Bytes::new(), true, &ctx.hlc,
        );
        apply_and_notify(&ctx.kv_state, &upd);
        dispatch_gossip_send(
            &ctx.gossip_txs, crate::framing::local_kv_wire_msg(&ctx, upd),
            ctx.node_id.id_hash(), ForwardHint::All,
        ).await;
    }
    for (i, key) in opacity_keys.iter().enumerate() {
        if opaque_written[i] {
            clear_opaque(key);
        }
    }
}

/// Writes (or tombstones) `grp/{group}/{node_id}` for this node, mirroring the
/// effect of `GossipAgent::{join_group, leave_group}` from a spawned task
/// without holding an agent reference.
pub(crate) fn emit_membership(ctx: &TaskCtx, own: &NodeId, group: &Arc<str>, leaving: bool) {
    use crate::framing::{dispatch_gossip_try_send, ForwardHint, make_gossip_update};
    use crate::store::apply_and_notify;
    let key: Arc<str> = Arc::from(format!("grp/{}/{}", group, own).as_str());
    let payload = if leaving { Bytes::new() } else { Bytes::from_static(&[1u8]) };
    let upd = make_gossip_update(
        &ctx.node_id, ctx.default_ttl, key, payload, leaving, &ctx.hlc,
    );
    apply_and_notify(&ctx.kv_state, &upd);
    dispatch_gossip_try_send(
        &ctx.gossip_txs, crate::framing::local_kv_wire_msg(ctx, upd),
        ctx.node_id.id_hash(), ForwardHint::All, &ctx.kv_state.dropped_frames,
    );
    if leaving {
        // Also update local boundary so signal admission stops immediately.
        ctx.signal_boundary.write().groups.remove(group);
    } else {
        ctx.signal_boundary.write().groups.insert(Arc::clone(group));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::make_gossip_update_stamped;
    use crate::store::{apply_and_notify, KvState};

    /// **A stale own advertisement is not projected into a group** (#734's review, finding 3). A node restarted
    /// over persisted `cap/` entries it no longer refreshes read them back as its own capabilities, so it joined
    /// the groups they match and projected a *fresh* `gcap/` that demand and wiring count. Its own check judges
    /// staleness only: an entry stamped ahead (its HLC ahead of the wall clock) is still its own.
    #[test]
    fn a_stale_own_advertisement_is_not_projected_into_a_group() {
        let kv = KvState::new(0);
        let own = NodeId::new("127.0.0.1", 9001).unwrap();
        let now = mycelium_core::sim_seam::wall_now_ms();
        for (name, written_ms) in [("live", now), ("ahead", now + 60_000), ("gone", now - 60_000)] {
            let entry = CapEntry { capability: Capability::new("demo", name), refresh_interval_ms: 1_000 };
            let key = format!("cap/{own}/demo/{name}");
            apply_and_notify(&kv, &make_gossip_update_stamped(&own, 5, Arc::from(key.as_str()),
                entry.encode(), false, crate::hlc::pack(written_ms, 0)));
        }
        let mut names: Vec<String> = own_capabilities(&kv, &own, now).into_iter().map(|c| c.name.to_string()).collect();
        names.sort();
        assert_eq!(names, vec!["ahead".to_string(), "live".to_string()], "a stale own ad was projected");
    }
}
