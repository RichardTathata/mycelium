//! Layer 2 — Signal / Boundary Mesh.
//!
//! Signals are ephemeral events that propagate epidemically to the entire cluster. Each node
//! holds a local [`Boundary`] (its receptor set) that decides whether it *acts* on an incoming
//! signal. Forwarding is always unconditional — the boundary only controls local delivery.
//!
//! Key types:
//! - [`SignalScope`] — Cluster (every node), Group (members only), Individual (one node)
//! - [`Signal`] — the delivered event: kind, scope, payload, sender, nonce
//! - [`AdvertiseHandle`] — cancels a periodic `advertise()` task on drop
//!
//! Well-known kind strings live in [`signal_kind`]. KV namespace conventions live in [`kv_ns`].
//!
//! All signal APIs are exposed directly on `GossipAgent` — there is no
//! separate Layer 2 wrapper type.

use crate::node_id::NodeId;
use crate::store::StoreEntry;
use ahash::AHashSet;
use bytes::Bytes;
use papaya::HashMap as PapayaMap;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::warn;

/// Default sender-log retention window (10 minutes).
///
/// Matches the default value of [`GossipConfig::signal_window_secs`].
/// Kept for doc-link references in `ConsensusConfig` and
/// `GossipConfig`. In application code, prefer
/// `GossipAgent::signal_window` — it reads the
/// operator-configured value. The live window stored on [`SignalHandlers`] is set from
/// `signal_window_secs` at agent construction and is used for all runtime eviction.
#[allow(dead_code)]
pub const SENDER_LOG_WINDOW: Duration = Duration::from_secs(600);

/// Most distinct signal kinds the sender log and the last-seen table hold at once (row B,
/// post-360 hardening). A kind is chosen by whoever sends the frame, so without a bound a peer
/// emitting random kinds grew both tables by one entry per kind. At the bound the least-recently
/// seen kinds make room, an eighth of the bound per pass (`SystemStats::signal_log_kinds_evicted`) —
/// except kinds this node subscribes to or has queried (`quorum*`, `last_signal`), which are never
/// evicted; a new kind is refused (`signal_log_kinds_refused`) only when every tracked kind is
/// exempt and it is not (#602's review, finding 3: refusing new kinds made a flood a false
/// negative for `quorum`).
pub const SIGNAL_LOG_MAX_KINDS: usize = 4096;

/// Most distinct senders one kind's sender log holds (row B). The log keeps each sender's
/// **latest** signal only — a quorum counts distinct senders, so earlier entries from the same
/// sender never changed an answer — and past the bound the senders heard from longest ago go, an
/// eighth of the bound per pass, so an active sender stays and an insert costs constant time
/// amortised. Forged senders can still displace quiet real ones by out-pacing them; a quorum over
/// forgeable senders was already forgeable. Worst case for the whole log:
/// `SIGNAL_LOG_MAX_KINDS × SIGNAL_LOG_MAX_SENDERS_PER_KIND` entries.
pub const SIGNAL_LOG_MAX_SENDERS_PER_KIND: usize = 1024;

/// Scope of a signal — determines which nodes **act** on it.
///
/// All nodes **forward** all signals regardless of scope (fully epidemic propagation).
/// The receiving node's [`Boundary`] decides whether to act, not whether to forward.
/// This mirrors the chemical signalling model: hormones flood the bloodstream;
/// only cells with the matching receptor respond.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SignalScope {
    /// Every node in the cluster acts.
    ///
    /// **Best-effort epidemic delivery.** Under high message volume the opacity
    /// mechanism may shed signals at boundaries before they propagate to all nodes.
    /// Do not use for coordination that requires exactly-once or guaranteed delivery
    /// — use application-level timers with gossip KV state propagation instead.
    Cluster,
    /// Only nodes that have joined the named group act.
    Group(Arc<str>),
    /// Only the named node acts.
    Individual(NodeId),
    /// Only nodes that have joined **any** of the named groups act (union membership).
    ///
    /// Used by `GossipAgent::cross_group_propose`
    /// to broadcast a ballot to all participants across multiple voting blocs in one shot.
    Groups(Vec<Arc<str>>),
}

/// A signal delivered to a local handler.
#[derive(Clone, Debug)]
pub struct Signal {
    /// Identifies the signal type and routes to registered handlers.
    pub kind:    Arc<str>,
    /// Scope set by the emitter — informational for handler logic.
    pub scope:   SignalScope,
    /// Application-defined payload bytes.
    pub payload: Bytes,
    /// Node that originally emitted this signal.
    pub sender:  NodeId,
    /// Random u64 used for network-level deduplication.
    pub nonce:   u64,
}

/// If `key` is a group-membership key belonging to `node_id_str`, returns the group name.
///
/// Matches keys of the form `grp/{group}/{node_id_str}` (live or tombstone). Returns `None`
/// for any other key.
pub fn parse_own_grp_key<'a>(key: &'a str, node_id_str: &str) -> Option<&'a str> {
    let inner = key.strip_prefix("grp/")?;
    let slash  = inner.rfind('/')?;
    if inner[slash + 1..] != *node_id_str { return None; }
    Some(&inner[..slash])
}

/// Returns the KV prefix for group membership keys: `grp/{group}/`.
///
/// Use this wherever a raw `format!("grp/{}/", group)` string would otherwise appear,
/// so all callers stay consistent with the [`kv_ns::GROUP`] namespace convention.
pub fn grp_prefix(group: &str) -> String {
    format!("grp/{}/", group)
}

/// Returns the KV key for a single node's group membership entry: `grp/{group}/{node_id}`.
pub fn grp_member_key(group: &str, node_id: &crate::node_id::NodeId) -> String {
    format!("{}{}", grp_prefix(group), node_id)
}

/// Local boundary filter.
///
/// Holds the set of groups this node has joined. `admits()` is O(1).
/// Mutated by `join_group` / `leave_group` (write path) and checked by the
/// connection handler (read path) via `Arc<RwLock<Boundary>>`.
pub struct Boundary {
    pub groups:  AHashSet<Arc<str>>,
    pub node_id: NodeId,
}

impl Boundary {
    pub fn new(node_id: NodeId) -> Self {
        Self { groups: AHashSet::new(), node_id }
    }

    /// Returns `true` if this node should act on a signal addressed to `scope`.
    #[inline]
    pub fn admits(&self, scope: &SignalScope) -> bool {
        match scope {
            SignalScope::Cluster => true,
            SignalScope::Group(name) => self.groups.contains(name),
            SignalScope::Individual(id) => *id == self.node_id,
            SignalScope::Groups(names) => names.iter().any(|n| self.groups.contains(n)),
        }
    }
}

/// Per-kind sender history shard type alias.
/// `(sender, received_at)`. The *interval* is what the replay kernel owns — see
/// `sim_seam::mono_elapsed`.
/// One kind's senders: each sender's **latest** signal, keyed by `NodeId::id_hash` (row B — a quorum
/// counts distinct senders, so earlier entries from the same sender never changed an answer, and
/// keeping one per sender is what lets the per-kind bound hold without a chatty sender pushing a
/// quiet one out).
type SenderEntries = ahash::AHashMap<u64, (NodeId, Instant)>;
type SenderLog = PapayaMap<Arc<str>, Arc<Mutex<SenderEntries>>>;

/// A total order on monotonic stamps, through the clock seam (`mono_before`).
fn mono_order(a: &Instant, b: &Instant) -> std::cmp::Ordering {
    if crate::sim_seam::mono_before(a, b) { std::cmp::Ordering::Less }
    else if crate::sim_seam::mono_before(b, a) { std::cmp::Ordering::Greater }
    else { std::cmp::Ordering::Equal }
}

/// Removes the `n` entries heard from longest ago, in one pass.
fn evict_oldest_senders(log: &mut SenderEntries, n: usize) {
    let mut by_age: Vec<(u64, Instant)> = log.iter().map(|(h, (_, t))| (*h, *t)).collect();
    let n = n.min(by_age.len());
    if n == 0 { return; }
    if n < by_age.len() {
        by_age.select_nth_unstable_by(n - 1, |a, b| mono_order(&a.1, &b.1));
    }
    for (h, _) in by_age.into_iter().take(n) {
        log.remove(&h);
    }
}

// ── SignalHandlers sub-types ──────────────────────────────────────────────────
//
// SignalHandlers used to bundle five concerns into one struct (handler-table
// fan-out, last-seen tracking, suppression, sender-log for quorum queries,
// and per-(kind, sender) rate-limiting for sys/quorum/ writes). Split into
// four focused types below so each owns one cluster of fields + methods:
//
//   HandlerTable      — register / fill_ratio / fan-out (the admission path)
//   SignalLog         — last_seen + sender_log + quorum + seed + trim
//   SuppressionTable  — refractory-period table (suppress / unsuppress / check)
//   QuorumEvidence    — sys/quorum/ rate-limited payload + trim
//
// `SignalHandlers` (further down) holds one of each and delegates. `deliver`
// orchestrates across them in a fixed order: record → suppression-check →
// fan-out. No behaviour change vs. the pre-split implementation.

/// A sender slot in [`HandlerTable`] with an optional sender-identity filter.
///
/// When `filter` is `None` the slot behaves like a bare `mpsc::Sender<Signal>`.
/// When `filter` is `Some`, only signals whose `signal.sender` is present in
/// the slice are forwarded to the channel. The `Arc<[NodeId]>` is shared across
/// clones so filtered registration is O(1) per extra receiver.
#[derive(Clone)]
struct FilteredSender {
    /// `None` = accept all senders. `Some(ids)` = accept only listed senders.
    filter: Option<Arc<[NodeId]>>,
    tx:     mpsc::Sender<Signal>,
    /// An observer (an SSE stream) rather than a subscriber that bears work: never counted in fill.
    tap:    bool,
}

impl FilteredSender {
    fn unfiltered(tx: mpsc::Sender<Signal>) -> Self {
        Self { filter: None, tx, tap: false }
    }
    fn filtered(tx: mpsc::Sender<Signal>, trusted: Arc<[NodeId]>) -> Self {
        Self { filter: Some(trusted), tx, tap: false }
    }
    fn is_closed(&self) -> bool { self.tx.is_closed() }
}

/// Handler fan-out registry: maps signal kind → list of `FilteredSender`.
///
/// papaya is the hot map type because `deliver` runs on every received signal
/// and registrations happen rarely. Value is `Arc<Vec<FilteredSender>>` so the
/// snapshot in `deliver_to_handlers` is a single atomic refcount increment;
/// the guard drops before the per-sender try_send loop.
struct HandlerTable {
    map:   PapayaMap<Arc<str>, Arc<Vec<FilteredSender>>>,
    /// Signals dropped to a full subscriber channel (row B).
    drops: std::sync::atomic::AtomicU64,
}

impl HandlerTable {
    fn new() -> Self { Self { map: PapayaMap::new(), drops: std::sync::atomic::AtomicU64::new(0) } }

    fn register_with_capacity(&self, kind: Arc<str>, cap: usize) -> mpsc::Receiver<Signal> {
        self.register_as(kind, cap, false)
    }

    fn register_as(&self, kind: Arc<str>, cap: usize, tap: bool) -> mpsc::Receiver<Signal> {
        let (tx, rx) = mpsc::channel(cap);
        // papaya re-invokes the closure when the entry changes concurrently
        // (another registration, or closed-sender eviction in
        // `deliver_to_handlers`), so it must be safe to run more than once:
        // clone the sender per invocation. A single-use `slot.take()` here
        // panicked on the retry (M2 Run-18 sweep finding; regression test:
        // `concurrent_same_kind_signal_registration_does_not_panic`).
        let fs = FilteredSender { tap, ..FilteredSender::unfiltered(tx) };
        self.map.pin().compute(kind, |existing| -> papaya::Operation<Arc<Vec<FilteredSender>>, ()> {
            match existing {
                None => papaya::Operation::Insert(Arc::new(vec![fs.clone()])),
                Some((_, arc)) => {
                    let mut v = (**arc).clone();
                    v.push(fs.clone());
                    papaya::Operation::Insert(Arc::new(v))
                }
            }
        });
        rx
    }

    fn register_with_filter(
        &self,
        kind:    Arc<str>,
        cap:     usize,
        trusted: Arc<[NodeId]>,
    ) -> mpsc::Receiver<Signal> {
        let (tx, rx) = mpsc::channel(cap);
        // Closure must be retry-safe — see `register_with_capacity`.
        let fs = FilteredSender::filtered(tx, trusted);
        self.map.pin().compute(kind, |existing| -> papaya::Operation<Arc<Vec<FilteredSender>>, ()> {
            match existing {
                None => papaya::Operation::Insert(Arc::new(vec![fs.clone()])),
                Some((_, arc)) => {
                    let mut v = (**arc).clone();
                    v.push(fs.clone());
                    papaya::Operation::Insert(Arc::new(v))
                }
            }
        });
        rx
    }

    /// How full this kind's subscribers are, for admission and opacity: the **least** full open
    /// subscriber's fill among those that bear work — taps (SSE streams) are not counted, so a fast
    /// observer cannot hide a saturated worker (#602's review, finding 4) — (row B, finding 25). It was the *most* full, so one subscriber that stopped
    /// reading — an SSE client, a serve stream — held its kind at 1.0 and the boundary admitted
    /// nothing of that kind for any subscriber on the node. A full subscriber now loses its own
    /// copies (dropped and counted in `deliver_to_handlers`); the kind sheds only when *every*
    /// subscriber is full, which is overload rather than one stalled reader.
    fn fill_ratio(&self, kind: &Arc<str>) -> f32 {
        let guard = self.map.pin();
        let Some(senders) = guard.get(kind.as_ref()) else { return 0.0 };
        let mut min_ratio: Option<f32> = None;
        for fs in senders.iter().filter(|fs| !fs.tap && !fs.is_closed()) {
            let ratio = 1.0_f32 - fs.tx.capacity() as f32 / fs.tx.max_capacity() as f32;
            min_ratio = Some(min_ratio.map_or(ratio, |m| m.min(ratio)));
        }
        min_ratio.unwrap_or(0.0).clamp(0.0, 1.0)
    }

    /// Whether any subscriber is registered for `kind`.
    /// Whether `kind` has an open subscriber that bears work (not a tap). Only such a kind is exempt
    /// from the sender log's eviction and writes `sys/quorum/` evidence (#602's re-review, findings 1
    /// and 4 — a `mesh:read` client opening SSE streams must not pin kinds).
    fn has_worker(&self, kind: &str) -> bool {
        self.map.pin().get(kind).is_some_and(|v| v.iter().any(|fs| !fs.tap && !fs.is_closed()))
    }

    /// Fans out a snapshot of senders for `signal.kind`. Sender-identity filters
    /// are checked per slot; closed senders are evicted lazily via CAS.
    fn deliver_to_handlers(&self, signal: &Signal) {
        let snapshot: Arc<Vec<FilteredSender>> = {
            let guard = self.map.pin();
            match guard.get(&*signal.kind) {
                Some(arc) => Arc::clone(arc),
                None => return,
            }
        };
        let mut has_closed = false;
        for fs in snapshot.iter() {
            // Sender-identity filter: skip this slot if the sender is not trusted.
            if let Some(ref trusted) = fs.filter
                && !trusted.iter().any(|id| id == &signal.sender) {
                    continue;
                }
            // Channel-readiness seam: a handler that cannot keep up drops a signal, and the drop
            // is a kernel decision so a recorded run reproduces it.
            match crate::sim_seam::chan_try_send(SIGNAL_CHAN, &fs.tx, signal.clone()) {
                crate::sim_seam::ChanVerdict::Sent => {}
                crate::sim_seam::ChanVerdict::Full => {
                    self.drops.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    #[cfg(feature = "metrics")]
                    metrics::counter!("gossip_signal_handler_drops_total").increment(1);
                    warn!(
                        kind = %signal.kind,
                        "Signal handler channel full; signal dropped. \
                         Handler is not draining fast enough — increase channel capacity \
                         via signal_rx_with_capacity or reduce signal rate.",
                    );
                }
                crate::sim_seam::ChanVerdict::Closed => { has_closed = true; }
            }
        }
        if has_closed {
            self.map.pin().compute(Arc::clone(&signal.kind), |existing| match existing {
                None => papaya::Operation::Abort(()),
                Some((_, arc)) => {
                    let filtered: Vec<_> = arc.iter().filter(|fs| !fs.is_closed()).cloned().collect();
                    if filtered.is_empty() {
                        papaya::Operation::Remove
                    } else {
                        papaya::Operation::Insert(Arc::new(filtered))
                    }
                }
            });
        }
    }
}

/// Per-kind history of admitted signals: when a kind was last seen and a
/// rolling window of `(sender, received_at)` for `quorum*` queries.
///
/// Distinct from `HandlerTable` because writes happen unconditionally — even
/// during suppression — so `quorum()` counts all received signals, not just
/// delivered ones.
struct SignalLog {
    last_seen:         PapayaMap<Arc<str>, Instant>,
    sender_log:        SenderLog,
    sender_log_window: Duration,
    /// New kinds not tracked because every tracked kind was exempt from eviction (row B).
    kinds_refused:     std::sync::atomic::AtomicU64,
    /// Kinds evicted, least recently seen first, to make room for a new one (#602's review, finding 3).
    kinds_evicted:     std::sync::atomic::AtomicU64,
    /// Kinds someone asked about (`quorum*`, `last_signal`, the `*_persistent` reads), with the
    /// monotonic-ns instant the pin lapses: a pin exempts its kind from eviction and lets it write
    /// `sys/quorum/` evidence until then — at least one `sender_log_window` after the last query, and
    /// at least the window the caller asked about (`quorum_persistent(kind, window)`; #602's round 3,
    /// finding 4). Pins were permanent before the re-review (finding 5). A watch renews its pin on
    /// every poll. Bounded by [`SIGNAL_LOG_MAX_KINDS`]: a full table drops lapsed pins first.
    queried:           PapayaMap<Arc<str>, u64>,
    /// At-capacity insertions that may still skip a scan, after one found nothing evictable
    /// (#602's re-review, finding 6) — valid only until `scan_credit_until` (mono ns), the earliest a
    /// pin seen by that scan lapses, so a lapsed pin makes its kind evictable at once (round 3, Q2).
    scan_credit:       std::sync::atomic::AtomicU64,
    scan_credit_until: std::sync::atomic::AtomicU64,
    /// One evictor at a time; a concurrent first sighting is admitted over the cap rather than
    /// evicting twice.
    evicting:          std::sync::atomic::AtomicBool,
    /// Full scans of the kind table made to find eviction candidates (diagnostics; #602's re-review).
    eviction_scans:    std::sync::atomic::AtomicU64,
    /// Records dropped after losing the race with eviction repeatedly (round 4; expected zero).
    records_dropped:   std::sync::atomic::AtomicU64,
}

impl SignalLog {
    fn new(sender_log_window: Duration) -> Self {
        Self {
            last_seen:  PapayaMap::new(),
            sender_log: PapayaMap::new(),
            sender_log_window,
            kinds_refused: std::sync::atomic::AtomicU64::new(0),
            kinds_evicted: std::sync::atomic::AtomicU64::new(0),
            queried:       PapayaMap::new(),
            scan_credit:   std::sync::atomic::AtomicU64::new(0),
            scan_credit_until: std::sync::atomic::AtomicU64::new(0),
            evicting:      std::sync::atomic::AtomicBool::new(false),
            eviction_scans: std::sync::atomic::AtomicU64::new(0),
            records_dropped: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Marks `kind` as asked about for one `sender_log_window`, exempting it from eviction.
    fn pin(&self, kind: &str) {
        self.pin_for(kind, self.sender_log_window);
    }

    /// Marks `kind` as asked about for `max(at_least, sender_log_window)` from now; an existing pin
    /// that lasts longer is kept.
    fn pin_for(&self, kind: &str, at_least: Duration) {
        let q = self.queried.pin();
        let now = crate::sim_seam::mono_now_ns();
        let span = at_least.max(self.sender_log_window).as_nanos().min(u64::MAX as u128) as u64;
        let until = now.saturating_add(span);
        if let Some(existing) = q.get(kind).copied() {
            if existing < until { q.insert(Arc::from(kind), until); }
            return;
        }
        if q.len() >= SIGNAL_LOG_MAX_KINDS {
            let lapsed: Vec<Arc<str>> = q.iter()
                .filter(|(_, until)| **until <= now)
                .map(|(k, _)| Arc::clone(k))
                .collect();
            for k in lapsed { q.remove(&k); }
            if q.len() >= SIGNAL_LOG_MAX_KINDS { return; }
        }
        q.insert(Arc::from(kind), until);
    }

    /// Whether `kind` holds a pin that has not lapsed.
    fn is_pinned(&self, kind: &str) -> bool {
        self.queried.pin().get(kind).is_some_and(|until| crate::sim_seam::mono_now_ns() < *until)
    }

    /// Whether the log tracks `kind` now (it was delivered, or seeded from `sys/quorum/`).
    fn tracks(&self, kind: &str) -> bool {
        self.last_seen.pin().contains_key(kind) || self.sender_log.pin().contains_key(kind)
    }

    /// Whether a kind the log does not track could only be admitted by eviction (evidence for it is
    /// not written — #602's review, finding 2). Seeded kinds live in the sender log without a
    /// last-seen stamp (`last_signal` stays a delivery fact), so both tables count.
    fn at_capacity(&self) -> bool {
        self.last_seen.pin().len().max(self.sender_log.pin().len()) >= SIGNAL_LOG_MAX_KINDS
    }

    /// Evicts, in one pass, up to an eighth of the cap's worth of the least-recently-seen kinds that
    /// are not exempt (a worker, a live query pin, or `keep`). One scan per `SIGNAL_LOG_MAX_KINDS / 8`
    /// new kinds while kinds are evictable; when a scan finds none, the next eighth's worth of
    /// at-capacity insertions skip the scan (`scan_credit`), so a table of exempt kinds costs a scan
    /// per 512 refusals, not per insert. One evictor at a time (`evicting`): a concurrent caller
    /// returns `None` and its kind is admitted over the cap. A kind is removed only if its last-seen
    /// stamp is still the one the scan read, so a kind recorded while the scan ran stays — whole.
    /// Returns how many went, or `None` when another evictor holds the pass.
    fn evict_kinds(&self, keep: &str, worker: &dyn Fn(&str) -> bool) -> Option<usize> {
        use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed};
        let credit = self.scan_credit.load(Relaxed);
        if credit > 0 && crate::sim_seam::mono_now_ns() < self.scan_credit_until.load(Relaxed) {
            self.scan_credit.store(credit - 1, Relaxed);
            return Some(0);
        }
        if self.evicting.compare_exchange(false, true, AcqRel, Acquire).is_err() {
            return None;
        }
        // Cleared on every exit, a panic included (#602's round 3, Q3): a flag left set would stop
        // eviction for good.
        struct Evicting<'a>(&'a std::sync::atomic::AtomicBool);
        impl Drop for Evicting<'_> {
            fn drop(&mut self) { self.0.store(false, std::sync::atomic::Ordering::Release); }
        }
        let _evicting = Evicting(&self.evicting);
        self.eviction_scans.fetch_add(1, Relaxed);
        let seen = self.last_seen.pin();
        let log = self.sender_log.pin();
        let evictable = |k: &Arc<str>| k.as_ref() != keep && !worker(k) && !self.is_pinned(k);
        let mut candidates: Vec<(Arc<str>, Instant, bool)> = seen.iter()
            .filter(|(k, _)| evictable(k))
            .map(|(k, t)| (Arc::clone(k), *t, true))
            .collect();
        // Seeded kinds with no delivery yet: aged by their newest entry.
        for (k, entries) in log.iter() {
            if seen.contains_key(k.as_ref()) || !evictable(k) { continue; }
            if let Some(t) = entries.lock().values().map(|(_, t)| *t).max_by(mono_order) {
                candidates.push((Arc::clone(k), t, false));
            }
        }
        let want = (SIGNAL_LOG_MAX_KINDS / 8).max(1);
        let n = want.min(candidates.len());
        if n < candidates.len() && n > 0 {
            candidates.select_nth_unstable_by(n - 1, |a, b| mono_order(&a.1, &b.1));
        }
        let mut gone = 0usize;
        for (k, stamp, delivered) in candidates.into_iter().take(n) {
            // Under the kind's own log lock (#602's round 4, finding 5): the recorder stamps and writes
            // under the same lock after checking its log is the table's, so the two cannot interleave —
            // either the recorder's newer stamp makes this compare fail, or this removal happens first
            // and the recorder, finding its log gone, writes into a fresh one.
            let held = log.get(k.as_ref()).cloned();
            let _guard = held.as_ref().map(|arc| arc.lock());
            let removed = if delivered {
                matches!(
                    seen.compute(Arc::clone(&k), |e| match e {
                        Some((_, t)) if *t == stamp => papaya::Operation::Remove,
                        _ => papaya::Operation::Abort(()),
                    }),
                    papaya::Compute::Removed(..)
                )
            } else {
                !seen.contains_key(k.as_ref())
            };
            if removed {
                if let Some(arc) = &held {
                    log.compute(Arc::clone(&k), |e| match e {
                        Some((_, cur)) if Arc::ptr_eq(cur, arc) => papaya::Operation::Remove,
                        _ => papaya::Operation::Abort(()),
                    });
                }
                gone += 1;
            }
        }
        if gone == 0 {
            // Skip the next eighth's scans — but only until the earliest pin among the tracked kinds
            // lapses, or one window passes, whichever is first.
            let now = crate::sim_seam::mono_now_ns();
            let queried = self.queried.pin();
            let earliest = seen.iter()
                .filter_map(|(k, _)| queried.get(k.as_ref()).copied())
                .filter(|until| *until > now)
                .min()
                .unwrap_or(u64::MAX);
            let window = self.sender_log_window.as_nanos().min(u64::MAX as u128) as u64;
            self.scan_credit_until.store(earliest.min(now.saturating_add(window)), Relaxed);
            self.scan_credit.store(want as u64, Relaxed);
        }
        self.kinds_evicted.fetch_add(gone as u64, Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("gossip_signal_log_kinds_evicted_total").increment(gone as u64);
        Some(gone)
    }

    /// Whether `kind` may be tracked: it already is, the log has room, or room is made by evicting the
    /// least-recently-seen kinds that no worker subscribes to and nobody has asked about lately (#602's
    /// review, finding 3 — refusing the new kind turned a memory bound into a false negative for
    /// `quorum`). Only when nothing is evictable is a new kind refused, unless it is itself exempt;
    /// refusals are counted. The bound is soft by concurrent first sightings.
    fn admit(&self, kind: &Arc<str>, worker: &dyn Fn(&str) -> bool) -> bool {
        if self.tracks(kind) || !self.at_capacity() {
            return true;
        }
        let exempt = worker(kind) || self.is_pinned(kind);
        match self.evict_kinds(kind, worker) {
            Some(0) if !exempt => {
                self.kinds_refused.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                #[cfg(feature = "metrics")]
                metrics::counter!("gossip_signal_log_kinds_refused_total").increment(1);
                false
            }
            _ => true,
        }
    }

    /// The per-kind entry for `kind`, created if absent (after [`admit`](Self::admit)).
    fn arc_for(&self, kind: &Arc<str>) -> Arc<Mutex<SenderEntries>> {
        let guard = self.sender_log.pin();
        if let Some(existing) = guard.get(kind.as_ref()) {
            return Arc::clone(existing);
        }
        let new_arc = Arc::new(Mutex::new(SenderEntries::default()));
        let mut result: Option<Arc<Mutex<SenderEntries>>> = None;
        guard.compute(Arc::clone(kind), |existing| match existing {
            Some((_, arc)) => { result = Some(Arc::clone(arc)); papaya::Operation::Abort(()) }
            None => { result = Some(Arc::clone(&new_arc)); papaya::Operation::Insert(Arc::clone(&new_arc)) }
        });
        result.expect("papaya compute always sets result via Abort or Insert")
    }

    /// Records that a signal of `kind` from `sender` was seen at `now` — when the kind is admitted.
    /// The last-seen stamp goes in **before** the sender entry: the evictor removes a kind only if
    /// its stamp is unchanged, so a kind recorded during a scan is either kept whole or re-created
    /// whole, never left as a stamp without a log (#602's re-review, finding 6). Keeps each sender's
    /// latest entry only; past [`SIGNAL_LOG_MAX_SENDERS_PER_KIND`] an eighth of the cap's worth of
    /// the senders heard from longest ago go in one pass.
    fn record(&self, kind: &Arc<str>, sender: NodeId, now: Instant, worker: &dyn Fn(&str) -> bool) {
        if !self.admit(kind, worker) { return; }
        // Stamp and write under the kind's log lock, after checking that log is still the table's
        // (#602's round 4, finding 5): the evictor removes a kind under the same lock and only if its
        // stamp is unchanged, so a record either lands whole in the live log or finds its log gone and
        // writes into a fresh one. A retry needs an eviction of this very kind in between; past a
        // handful the record is counted and dropped, never silently.
        for _ in 0..16 {
            let arc = self.arc_for(kind);
            let mut log = arc.lock();
            if !self.sender_log.pin().get(kind.as_ref()).is_some_and(|held| Arc::ptr_eq(held, &arc)) {
                continue;
            }
            self.last_seen.pin().insert(Arc::clone(kind), now);
            log.insert(sender.id_hash(), (sender, now));
            if log.len() > SIGNAL_LOG_MAX_SENDERS_PER_KIND {
                evict_oldest_senders(&mut log, (SIGNAL_LOG_MAX_SENDERS_PER_KIND / 8).max(1));
            }
            return;
        }
        self.records_dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        warn!(kind = %kind, "signal log: a record lost the race with eviction 16 times and was dropped");
    }

    fn last_signal(&self, kind: &str) -> Option<Instant> {
        self.pin(kind);
        self.last_seen.pin().get(kind).copied()
    }

    fn quorum(&self, kind: &str, min_senders: usize, window: Duration) -> bool {
        self.pin(kind);
        let Some(arc) = self.sender_log.pin().get(kind).map(Arc::clone) else { return false };
        let log = arc.lock();
        // One entry per sender (keyed by `id_hash`), so a count of fresh entries is a count of
        // distinct senders.
        let mut distinct = 0usize;
        for (_, received_at) in log.values() {
            if crate::sim_seam::mono_elapsed(received_at) > window { continue; }
            distinct += 1;
            if distinct >= min_senders { return true; }
        }
        false
    }

    fn quorum_for_group(
        &self,
        kind:          &str,
        member_hashes: &AHashSet<u64>,
        min_senders:   usize,
        window:        Duration,
    ) -> bool {
        self.pin(kind);
        let Some(arc) = self.sender_log.pin().get(kind).map(Arc::clone) else { return false };
        let log = arc.lock();
        let mut distinct = 0usize;
        for (hash, (_, received_at)) in log.iter() {
            if crate::sim_seam::mono_elapsed(received_at) > window { continue; }
            if !member_hashes.contains(hash) { continue; }
            distinct += 1;
            if distinct >= min_senders { return true; }
        }
        false
    }

    fn seed(&self, kind: Arc<str>, sender: NodeId, age_ms: u64) {
        if age_ms > self.sender_log_window.as_millis() as u64 { return; }
        // `age_ms` before now. `Instant` represents a point before process start natively, which is
        // what this needs: `warm_quorum_from_layer1` calls it at *startup*.
        let received_at = Instant::now()
            .checked_sub(Duration::from_millis(age_ms))
            .unwrap_or_else(Instant::now);
        // Seeds come from `sys/quorum/` records, whose kinds were chosen by senders too: the same
        // bound applies.
        if !self.admit(&kind, &|_| false) { return; }
        let arc = self.arc_for(&kind);
        let mut log = arc.lock();
        if log.len() >= SIGNAL_LOG_MAX_SENDERS_PER_KIND && !log.contains_key(&sender.id_hash()) {
            return;
        }
        let newer = log.get(&sender.id_hash())
            .is_none_or(|(_, held)| crate::sim_seam::mono_before(held, &received_at));
        if newer {
            log.insert(sender.id_hash(), (sender, received_at));
        }
    }

    /// Evicts sender-log entries older than `window` and drops kinds whose log becomes empty.
    /// `last_seen` (the kind table) is bounded by eviction at insert, not here, so `last_signal` keeps
    /// answering for a kind until it is evicted.
    fn trim(&self, window: Duration) {
        let cutoff = Instant::now().checked_sub(window);
        let fresh = |t: &Instant| cutoff.is_none_or(|c| crate::sim_seam::mono_before(&c, t));
        let to_remove: Vec<Arc<str>> = {
            let guard = self.sender_log.pin();
            guard.iter()
                .filter_map(|(kind, arc)| {
                    let mut log = arc.lock();
                    log.retain(|_, (_, t)| fresh(t));
                    if log.is_empty() { Some(Arc::clone(kind)) } else { None }
                })
                .collect()
        };
        let guard = self.sender_log.pin();
        for kind in to_remove {
            guard.remove(&kind);
        }
    }
}

/// Per-kind refractory periods. `is_suppressed_at` is called once per
/// `deliver` so the path is on the hot side; papaya keeps reads lock-free.
struct SuppressionTable {
    /// Suppressed until this monotonic-nanosecond reading.
    suppressed: PapayaMap<Arc<str>, Instant>,
}

impl SuppressionTable {
    fn new() -> Self { Self { suppressed: PapayaMap::new() } }

    fn suppress(&self, kind: Arc<str>, until: Instant) {
        self.suppressed.pin().insert(kind, until);
    }

    fn unsuppress(&self, kind: &str) {
        self.suppressed.pin().remove(kind);
    }

    /// `now` is plumbed in so `deliver` uses the same instant for both the
    /// log record and the suppression check, avoiding two clock reads per delivery.
    fn is_suppressed_at(&self, kind: &str, now: Instant) -> bool {
        self.suppressed.pin().get(kind)
            // Seamed: both sides are stamps this process took, so the *verdict* is what a replay
            // must reproduce (`sim_seam::mono_before`).
            .map(|until| crate::sim_seam::mono_before(&now, until))
            .unwrap_or(false)
    }

    fn is_suppressed(&self, kind: &str) -> bool {
        self.is_suppressed_at(kind, Instant::now())
    }
}

/// Tracks the last time a `sys/quorum/` entry was written for each
/// `{kind}/{sender}` key. Used to rate-limit Layer-I quorum-evidence writes
/// to one per second per pair without reading `KvState`.
struct QuorumEvidence {
    quorum_written: PapayaMap<Arc<str>, Instant>,
    /// Senders each kind has written evidence for, with when, at most
    /// [`SIGNAL_LOG_MAX_SENDERS_PER_KIND`] per kind (#602's round 3, finding 2). The set **slides**
    /// (round 4, finding 3): past the cap the senders written longest ago give way, an eighth per
    /// pass, so a kind never stops writing; and a kind that has neither a worker nor a live pin loses
    /// its set at the next trim. A sender is entered only when its write happens. What this bounds is
    /// the memory and — with the rate table — the write rate; `sys/quorum/` keys already written stay.
    senders_written: PapayaMap<Arc<str>, Arc<PapayaMap<u64, Instant>>>,
    /// Evidence writes skipped by either bound (diagnostics).
    skipped: std::sync::atomic::AtomicU64,
}

impl QuorumEvidence {
    fn new() -> Self {
        Self { quorum_written: PapayaMap::new(), senders_written: PapayaMap::new(), skipped: std::sync::atomic::AtomicU64::new(0) }
    }

    fn skip(&self) -> Option<(Arc<str>, Bytes)> {
        self.skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("gossip_quorum_evidence_skipped_total").increment(1);
        None
    }

    fn payload(&self, kind: &Arc<str>, sender: &NodeId) -> Option<(Arc<str>, Bytes)> {
        // Two bounds (#602's review finding 2; round 3 finding 2). Per kind: evidence keys for at most
        // the log's per-kind sender cap. Overall: the rate-limit table holds entries under a second
        // old and at most `SIGNAL_LOG_MAX_KINDS` of them — older ones are dropped when it is full, and
        // a write that would need a slot past that is **skipped**, never written unremembered, so
        // evidence is never written at line rate. The cost, stated: a burst of more than 4096 distinct
        // (kind, sender) pairs within one second loses some evidence for that second.
        let now = Instant::now();
        let quorum_key: Arc<str> = Arc::from(
            format!("{}{}/{}", kv_ns::QUORUM, kind, sender).as_str()
        );
        let held = self.quorum_written.pin();
        let should_write = held
            .get(&quorum_key)
            .map(|last| crate::sim_seam::mono_span(last, &now) > Duration::from_secs(1))
            .unwrap_or(true);
        if !should_write {
            return None;
        }
        if held.len() >= SIGNAL_LOG_MAX_KINDS && !held.contains_key(&quorum_key) {
            let stale: Vec<Arc<str>> = held.iter()
                .filter(|(_, last)| crate::sim_seam::mono_span(last, &now) > Duration::from_secs(1))
                .map(|(k, _)| Arc::clone(k))
                .collect();
            for k in stale { held.remove(&k); }
        }
        if held.len() >= SIGNAL_LOG_MAX_KINDS && !held.contains_key(&quorum_key) {
            return self.skip();
        }
        // The write happens: enter the sender in its kind's set (sliding past the cap).
        {
            let per_kind = self.senders_written.pin();
            let set = match per_kind.get(kind.as_ref()) {
                Some(set) => Arc::clone(set),
                None => {
                    let fresh = Arc::new(PapayaMap::new());
                    let mut held_set = None;
                    per_kind.compute(Arc::clone(kind), |e| match e {
                        Some((_, set)) => { held_set = Some(Arc::clone(set)); papaya::Operation::Abort(()) }
                        None => { held_set = Some(Arc::clone(&fresh)); papaya::Operation::Insert(Arc::clone(&fresh)) }
                    });
                    held_set.expect("compute sets it on both arms")
                }
            };
            let senders = set.pin();
            if !senders.contains_key(&sender.id_hash()) && senders.len() >= SIGNAL_LOG_MAX_SENDERS_PER_KIND {
                let mut by_age: Vec<(u64, Instant)> = senders.iter().map(|(h, t)| (*h, *t)).collect();
                let n = (SIGNAL_LOG_MAX_SENDERS_PER_KIND / 8).max(1).min(by_age.len());
                if n < by_age.len() {
                    by_age.select_nth_unstable_by(n - 1, |a, b| mono_order(&a.1, &b.1));
                }
                for (h, _) in by_age.into_iter().take(n) { senders.remove(&h); }
            }
            senders.insert(sender.id_hash(), now);
        }
        held.insert(Arc::clone(&quorum_key), now);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH).unwrap_or_default()
            .as_millis() as u64;
        Some((quorum_key, Bytes::copy_from_slice(&now_ms.to_le_bytes())))
    }

    /// Drops the sender sets of kinds that may no longer write evidence (no worker, no live pin).
    fn prune_kinds(&self, may_write: &dyn Fn(&str) -> bool) {
        let per_kind = self.senders_written.pin();
        let gone: Vec<Arc<str>> = per_kind.iter()
            .filter(|(k, _)| !may_write(k))
            .map(|(k, _)| Arc::clone(k))
            .collect();
        for k in gone { per_kind.remove(&k); }
    }

    fn trim(&self, window: Duration, now: Instant) {
        let stale: Vec<Arc<str>> = self.quorum_written.pin()
            .iter()
            .filter_map(|(k, last)| {
                if crate::sim_seam::mono_span(last, &now) > window { Some(Arc::clone(k)) } else { None }
            })
            .collect();
        let guard = self.quorum_written.pin();
        for k in stale {
            guard.remove(&k);
        }
    }
}

// ── SignalHandlers façade ─────────────────────────────────────────────────────

/// Fan-out registry plus auxiliary state for signal delivery.
///
/// Internally composed of four focused sub-types ([`HandlerTable`],
/// [`SignalLog`], [`SuppressionTable`], [`QuorumEvidence`]). The pub
/// surface stays unchanged: every method delegates to the appropriate
/// sub-type. `deliver` orchestrates across all four in a fixed order:
/// record → suppression-check → fan-out.
pub struct SignalHandlers {
    handlers:    HandlerTable,
    log:         SignalLog,
    suppression: SuppressionTable,
    evidence:    QuorumEvidence,
}

impl SignalHandlers {
    pub fn new(sender_log_window: Duration) -> Self {
        Self {
            handlers:    HandlerTable::new(),
            log:         SignalLog::new(sender_log_window),
            suppression: SuppressionTable::new(),
            evidence:    QuorumEvidence::new(),
        }
    }

    /// Returns a new `mpsc::Receiver<Signal>` for `kind` with the default channel depth (256).
    /// Multiple calls for the same kind produce independent receivers.
    pub fn register(&self, kind: Arc<str>) -> mpsc::Receiver<Signal> {
        self.handlers.register_with_capacity(kind, 256)
    }

    /// Returns a new `mpsc::Receiver<Signal>` for `kind` with a caller-specified channel depth.
    pub fn register_with_capacity(&self, kind: Arc<str>, cap: usize) -> mpsc::Receiver<Signal> {
        self.handlers.register_with_capacity(kind, cap)
    }

    /// Registers a **tap** for `kind`: an observer that receives what is admitted — an SSE stream to
    /// a dashboard — and does no work the node is responsible for. A tap loses its own signals when
    /// full and never counts toward the kind's fill (#602's review, finding 4): admission and opacity
    /// read the subscribers that bear work, so a fast tap cannot hide a saturated worker.
    pub fn register_tap(&self, kind: Arc<str>, cap: usize) -> mpsc::Receiver<Signal> {
        self.handlers.register_as(kind, cap, true)
    }

    /// Senders held in `kind`'s evidence set (tests and diagnostics).
    pub fn evidence_senders(&self, kind: &str) -> usize {
        self.evidence.senders_written.pin().get(kind).map_or(0, |s| s.pin().len())
    }

    /// Kinds holding an evidence sender set (tests and diagnostics).
    pub fn evidence_kinds(&self) -> usize {
        self.evidence.senders_written.pin().len()
    }

    /// `sys/quorum/` evidence rate-limit entries held (tests and diagnostics).
    pub fn evidence_entries(&self) -> usize {
        self.evidence.quorum_written.pin().len()
    }

    /// Returns a receiver that only delivers signals whose `sender` is in `trusted`.
    /// An empty `trusted` list is equivalent to `register` — no filter overhead.
    pub fn register_from(
        &self,
        kind:    Arc<str>,
        trusted: Vec<NodeId>,
    ) -> mpsc::Receiver<Signal> {
        if trusted.is_empty() {
            return self.handlers.register_with_capacity(kind, 256);
        }
        let trusted_arc: Arc<[NodeId]> = trusted.into();
        self.handlers.register_with_filter(kind, 256, trusted_arc)
    }

    /// How full `kind`'s subscribers are: the **least** full open subscriber's fill, `0.0` with
    /// none. One full subscriber loses its own signals (counted in [`handler_drops`](Self::handler_drops))
    /// rather than holding the kind at 1.0 for every other subscriber (row B); the kind reads full
    /// only when every subscriber is.
    pub fn fill_ratio(&self, kind: &Arc<str>) -> f32 {
        self.handlers.fill_ratio(kind)
    }

    /// Fans out `signal` to all receivers registered for `signal.kind`.
    /// Closed senders are removed lazily. Full channels log a warning and drop the signal.
    /// Records the delivery time for [`last_signal`](Self::last_signal) regardless
    /// of whether any handlers are registered.
    ///
    /// Hot-path design: records into [`SignalLog`] first (unconditional —
    /// quorum counting includes suppressed kinds), checks the
    /// [`SuppressionTable`] using the same `now` (one clock read per delivery),
    /// then delegates fan-out to [`HandlerTable`].
    pub fn deliver(&self, signal: &Signal) {
        let now = Instant::now();
        // A kind this node subscribes to is always tracked; a kind only a sender chose is tracked
        // while the log has room (row B).
        self.log.record(&signal.kind, signal.sender.clone(), now, &|k| self.handlers.has_worker(k));
        if self.suppression.is_suppressed_at(&signal.kind, now) {
            #[cfg(feature = "metrics")]
            metrics::counter!("gossip_signals_rejected_total").increment(1);
            return;
        }
        #[cfg(feature = "metrics")]
        metrics::counter!("gossip_signals_delivered_total", "kind" => signal.kind.to_string()).increment(1);
        self.handlers.deliver_to_handlers(signal);
    }

    /// Distinct kinds the sender log and last-seen table hold (the larger of the two).
    pub fn log_kinds_tracked(&self) -> usize {
        self.log.sender_log.pin().len().max(self.log.last_seen.pin().len())
    }

    /// Entries the sender log holds for `kind`.
    pub fn log_entries(&self, kind: &str) -> usize {
        self.log.sender_log.pin().get(kind).map_or(0, |a| a.lock().len())
    }

    /// Signals dropped because a subscriber's channel was full, summed over every subscriber.
    pub fn handler_drops(&self) -> u64 {
        self.handlers.drops.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// New kinds the sender log declined to track because it held [`SIGNAL_LOG_MAX_KINDS`] kinds and
    /// every one was exempt from eviction (subscribed or queried).
    pub fn log_kinds_refused(&self) -> u64 {
        self.log.kinds_refused.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Marks `kind` as asked about for one sender-log window: exempt from eviction and writing
    /// `sys/quorum/` evidence. The `*_persistent` reads call it, so a node that reads a kind's
    /// evidence also contributes to it.
    pub fn pin_kind(&self, kind: &str) {
        self.log.pin(kind);
    }

    /// Like [`pin_kind`](Self::pin_kind), for at least `at_least` — what a reader asking about a
    /// window longer than the sender-log window needs (`quorum_persistent(kind, window)`).
    pub fn pin_kind_for(&self, kind: &str, at_least: Duration) {
        self.log.pin_for(kind, at_least);
    }

    /// Scans of the kind table made to find eviction candidates (diagnostics).
    pub fn log_eviction_scans(&self) -> u64 {
        self.log.eviction_scans.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Kinds the sender log evicted, least recently seen first, to make room for new ones.
    pub fn log_kinds_evicted(&self) -> u64 {
        self.log.kinds_evicted.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Returns when this node last admitted a signal of `kind`, or `None` if never.
    pub fn last_signal(&self, kind: &str) -> Option<Instant> {
        self.log.last_signal(kind)
    }

    pub fn suppress(&self, kind: Arc<str>, until: Instant) {
        self.suppression.suppress(kind, until);
    }

    pub fn unsuppress(&self, kind: &str) {
        self.suppression.unsuppress(kind);
    }

    pub fn is_suppressed(&self, kind: &str) -> bool {
        self.suppression.is_suppressed(kind)
    }

    /// Removes sender-log entries and rate-limit entries older than `window`,
    /// then drops kinds whose log has become empty. Called from the GC task
    /// on each GC tick.
    pub fn trim_sender_log(&self, window: Duration) {
        self.log.trim(window);
        self.evidence.trim(window, Instant::now());
        self.evidence.prune_kinds(&|k| self.handlers.has_worker(k) || self.log.is_pinned(k));
    }

    /// Seeds the sender log with a past entry reconstructed from a
    /// `sys/quorum/` Layer I record (used by
    /// `GossipAgent::warm_quorum_from_layer1`).
    pub fn seed_sender_log(&self, kind: Arc<str>, sender: NodeId, age_ms: u64) {
        self.log.seed(kind, sender, age_ms);
    }

    /// Returns `true` when at least `min_senders` distinct [`NodeId`]s have had a
    /// signal of `kind` delivered within `window`.
    pub fn quorum(&self, kind: &str, min_senders: usize, window: Duration) -> bool {
        self.log.quorum(kind, min_senders, window)
    }

    /// Like [`quorum`](Self::quorum) but only counts senders whose `id_hash()` is in
    /// `member_hashes`. **Not suitable for per-ballot consensus vote counting** —
    /// the sender log is keyed by `(kind, sender)` only, not `(slot, ballot)`.
    pub fn quorum_for_group(
        &self,
        kind:          &str,
        member_hashes: &AHashSet<u64>,
        min_senders:   usize,
        window:        Duration,
    ) -> bool {
        self.log.quorum_for_group(kind, member_hashes, min_senders, window)
    }

    /// Returns the quorum-evidence key and value to write, or `None` if the existing entry is less
    /// than 1 second old (rate-limit to prevent gossip churn) — or if the kind is not worth
    /// persisting: no local worker subscribes to it and nobody here has asked about it within the
    /// sender-log window (2.32, #602's re-review). A node that only relays a kind no longer writes
    /// `sys/quorum/` keys for it; one that subscribes or queries (including `quorum_persistent` and
    /// `last_signal_persistent`, which pin the kind) does.
    pub fn quorum_evidence_payload(
        &self,
        kind:   &Arc<str>,
        sender: &NodeId,
    ) -> Option<(Arc<str>, Bytes)> {
        // KV evidence only for a kind worth persisting: one a local worker subscribes to, or that was
        // asked about within the window (#602's re-review, finding 1). A kind only a sender chose —
        // a flood of random kinds — writes nothing to the WAL or the mesh.
        if !self.handlers.has_worker(kind) && !self.log.is_pinned(kind) {
            return None;
        }
        self.evidence.payload(kind, sender)
    }
}

// ── Boundary reconciliation ───────────────────────────────────────────────────

/// Reconciles `Boundary::groups` from `grp/{group}/{node_id_str}` entries in the store.
///
/// Live entries insert into `groups`; tombstoned entries remove. Called at startup
/// (`rehydrate_boundary_from_kv`) and periodically by the GC task as a catch-all
/// for membership updates missed by the push-based path in the connection handler.
///
/// The caller holds the `RwLock` write guard and passes `&mut Boundary` directly,
/// keeping locking policy with the caller.
pub fn reconcile_boundary_from_store(
    store:       &PapayaMap<Arc<str>, StoreEntry>,
    boundary:    &mut Boundary,
    node_id_str: &str,
) {
    let mut to_insert: Vec<Arc<str>> = Vec::new();
    let mut to_remove: Vec<Arc<str>> = Vec::new();
    {
        let guard = store.pin();
        for (key, entry) in guard.iter() {
            let Some(group) = parse_own_grp_key(key, node_id_str) else { continue };
            if entry.data.is_some() {
                to_insert.push(Arc::from(group));
            } else {
                to_remove.push(Arc::from(group));
            }
        }
    }
    for g in to_insert { boundary.groups.insert(g); }
    for g in &to_remove { boundary.groups.remove(g.as_ref()); }
}

// ── Pheromone trail ───────────────────────────────────────────────────────────

/// Pheromone load state written to Layer I by `GossipAgent::manage_opacity`.
///
/// Key convention: `sys/load/{node_id}/{kind}` (see [`kv_ns::LOAD`]).
/// Encoded with the in-tree fixed-int `serde_fixint` codec.
/// An absent key means the node is transparent (not overloaded) for that kind.
/// Tombstoned automatically when `BOUNDARY_TRANSPARENT` is emitted.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LoadState {
    /// Handler-channel fill ratio at time of writing (0.0–1.0).
    pub fill_ratio: f32,
    /// Whether [`BOUNDARY_OPAQUE`](signal_kind::BOUNDARY_OPAQUE) has been emitted.
    pub is_opaque: bool,
    /// Milliseconds since Unix epoch when this entry was written.
    ///
    /// Readers discard entries where `now_ms − written_at_ms` exceeds their
    /// chosen evaporation window (should be ≤ [`SENDER_LOG_WINDOW`]).
    pub written_at_ms: u64,
}

pub fn encode_load_state(s: &LoadState) -> Bytes {
    Bytes::from(crate::serde_fixint::to_vec(s).unwrap_or_default())
}

pub fn decode_load_state(b: &Bytes) -> Option<LoadState> {
    let mut ls: LoadState = crate::serde_fixint::from_slice(b).ok()?;
    // Clamp the peer-supplied `fill_ratio` to [0,1] (NaN → 0). It is gossiped raw under `sys/load/`
    // (no write guard), and consumers multiply/compare it — the consensus retry-jitter turned an
    // unclamped `1e30`/`Inf` into `(1e30 * jitter) as u64` = `u64::MAX` → a ~584-million-year
    // `Duration::from_millis` sleep that stalls a poisoned node's consensus (audit 2026-07-15 pass 5).
    ls.fill_ratio = if ls.fill_ratio.is_nan() { 0.0 } else { ls.fill_ratio.clamp(0.0, 1.0) };
    Some(ls)
}

/// Cancels the associated `advertise` task on drop.
///
/// Obtain one from `GossipAgent::advertise`. The task also exits automatically
/// when the agent shuts down, even if this handle is still live.
pub struct AdvertiseHandle {
    pub _cancel: tokio::sync::oneshot::Sender<()>,
}

/// Cancels the associated `watch` task on drop.
///
/// Obtain one from `GossipAgent::watch`. The task also exits automatically
/// when the agent shuts down, even if this handle is still live.
pub struct WatchHandle {
    pub _cancel: tokio::sync::oneshot::Sender<()>,
}

/// Cancels the associated `manage_opacity` governor
/// task on drop.
///
/// Obtain one from `GossipAgent::manage_opacity` or
/// `GossipAgent::manage_opacity_gated`. The task also exits automatically when
/// the agent shuts down, even if this handle is still live.
pub struct OpacityHandle {
    pub _cancel: tokio::sync::oneshot::Sender<()>,
}

/// Application hint for the opacity governor.
///
/// All fields have documented defaults; use [`OpacityHint::default()`] and override
/// only what you need.
#[derive(Clone, Debug)]
pub struct OpacityHint {
    /// Suggested threshold at which `BOUNDARY_OPAQUE` should be emitted (0.0–1.0).
    ///
    /// The library clamps this to `[0.4, 0.95]` and reduces it further when the
    /// fill rate is rising quickly (trend adaptation). Default: `0.75`.
    pub threshold:  f32,
    /// How far fill must drop below `threshold` before `BOUNDARY_TRANSPARENT` is emitted.
    ///
    /// Prevents oscillation at the threshold boundary. Default: `0.20`.
    pub hysteresis: f32,
    /// Payload attached to the `BOUNDARY_OPAQUE` signal.
    ///
    /// Useful for carrying application-defined context (e.g. a reason string or
    /// estimated drain time in milliseconds). Default: empty.
    pub payload:    bytes::Bytes,
    /// Minimum interval between the previous boundary transition and a **release**
    /// (`BOUNDARY_TRANSPARENT`), in milliseconds — the control contract's spacing for this
    /// actuator (v2.8.0, `docs/design/adaptive-stability.md` §9 row 4b). Only the release is
    /// spaced: going opaque is protective shedding and is never held. `0` disables it.
    /// Default: `1_000` (ten governor ticks).
    pub release_spacing_ms: u64,
}

impl Default for OpacityHint {
    fn default() -> Self {
        Self {
            threshold:  0.75,
            hysteresis: 0.20,
            payload:    bytes::Bytes::new(),
            release_spacing_ms: 1_000,
        }
    }
}

/// Snapshot of governor state passed to the application gate on each tick.
#[derive(Clone, Debug)]
pub struct OpacityState {
    /// Current fill ratio of the monitored kind's handler channel (0.0–1.0).
    pub fill_ratio:          f32,
    /// Threshold the library computed after applying trend adaptation to the hint.
    pub effective_threshold: f32,
    /// Fill change since the previous tick (positive = filling, negative = draining).
    pub trend:               f32,
    /// Whether `BOUNDARY_OPAQUE` has been emitted and not yet cleared.
    pub is_opaque:           bool,
}

/// Well-known signal kind string constants.
///
/// These are conventions, not protocol requirements. Applications are free to
/// define their own signal kinds alongside or instead of these.
pub mod signal_kind {
    /// Invoke a contract — payload is the serialized invocation request.
    pub const INVOKE:               &str = "invoke";
    /// Result of a contract invocation.
    ///
    /// **Nonce convention**: the first 8 bytes of the payload carry a little-endian u64
    /// correlation nonce that matches the first 8 bytes of the originating
    /// [`INVOKE`] or [`INVOKE_BULK`] payload. Use `GossipAgent::request`
    /// on the caller side to generate and match the nonce automatically.
    pub const INVOKE_RESULT:        &str = "invoke.result";
    /// Bulk-invoke signal. The sender emits this kind to trigger a batch operation
    /// on a group of peers. Payload carries a ticket/correlation ID; the actual
    /// data transfer is the responsibility of the application's Layer 3 transport
    /// (HTTP, gRPC, shared storage, etc. — not provided by this library).
    ///
    /// Responders reply with [`INVOKE_RESULT`] echoing the ticket in the first 8
    /// payload bytes so the initiator can correlate via `GossipAgent::request`.
    pub const INVOKE_BULK:          &str = "invoke.bulk";
    /// A contract has become available at `sender`.
    pub const CONTRACT_AVAILABLE:   &str = "contract.available";
    /// A contract has been withdrawn from `sender`.
    pub const CONTRACT_WITHDRAWN:   &str = "contract.withdrawn";
    /// Cluster lifecycle event (join / leave / restart).
    pub const CLUSTER_EVENT:        &str = "cluster.event";
    /// Liveness probe.
    pub const HEALTH_PROBE:         &str = "health.probe";
    /// Liveness response.
    pub const HEALTH_ACK:           &str = "health.ack";
    /// Node is entering an opaque state — load is high, incoming work being shed.
    /// Payload: application-defined (e.g. reason string, ETA milliseconds).
    /// Upstream nodes should drain service-level connections to `sender` on receipt.
    pub const BOUNDARY_OPAQUE:      &str = "boundary.opaque";
    /// Node has cleared its load and resumed normal signal admission.
    /// The pheromone trail (`sys/load/{node_id}/{kind}`) is tombstoned immediately.
    pub const BOUNDARY_TRANSPARENT: &str = "boundary.transparent";
    /// Generic RPC reply. The correlation nonce in the first 8 bytes of the
    /// originating request payload is echoed at the start of this payload.
    /// Use `GossipAgent::rpc_call` /
    /// `GossipAgent::rpc_respond` to handle
    /// the nonce automatically.
    pub const RPC_RESULT: &str = "rpc.result";
    /// Reply to an [`INVOKE_BULK`] call. Same nonce-prefix convention as
    /// [`RPC_RESULT`] but on a dedicated kind so bulk and RPC reply handlers
    /// do not compete for the same signal dispatch slot.
    pub const BULK_RESULT: &str = "bulk.result";
    /// MCP tool invocation — payload is a JSON-RPC 2.0 request body.
    /// Replies are sent as [`RPC_RESULT`] signals back to the caller.
    pub const MCP_INVOKE: &str = "mcp.invoke";

    /// LLM prompt skill invocation — payload is JSON `{"prompt":"{ns}/{name}","input":"...","context":{...}}`.
    /// Replies are sent as [`RPC_RESULT`] signals back to the caller.
    pub const LLM_INVOKE: &str = "llm.invoke";

    /// Agent state transition notification.
    /// Payload: `{"node": "<id>", "from": "<state>", "to": "<state>"}`.
    /// Emitted by `AgentStateMachine::transition`
    /// after every committed transition.
    pub const AGENT_STATE: &str = "agent.state";

    /// Agent requesting supervisor approval before an `Invoking` transition.
    /// Payload: `{"node": "<id>", "tool": "<name>"}`.
    /// Supervisors reply with [`AGENT_VETO`] to block the transition or stay
    /// silent to approve (default — no reply within 30 s = approved).
    pub const AGENT_APPROVE: &str = "agent.approve";

    /// Supervisor veto of a pending `Invoking` transition.
    /// Payload: `{"tool": "<name>"}`. Must arrive within 30 s of the
    /// corresponding [`AGENT_APPROVE`] signal.
    pub const AGENT_VETO: &str = "agent.veto";
}

/// Well-known KV key namespace prefixes for pheromone trail and membership state.
///
/// These conventions structure entries in the Layer 1 store. The store is the shared
/// medium — pheromone trails written here are persistent, anti-entropy synced, and
/// readable by any node at any time without signal handlers or local caches.
///
/// **Namespace protection**: entries under `sys/` are written exclusively by the
/// library. Applications must not write to `sys/load/`, `sys/quorum/`, or any other
/// `sys/` sub-namespace — doing so will corrupt pheromone trails and quorum evidence,
/// leading to incorrect opacity decisions and stale quorum reads. Use the `grp/`,
/// `svc/`, `consensus/`, and application-defined namespaces for application data.
pub mod kv_ns {
    /// Pheromone trail namespace (library-internal — do not write from application code).
    ///
    /// Key: `sys/load/{node_id}/{kind}`. Value: bincode-encoded `LoadState`.
    ///
    /// Written automatically by `GossipAgent::manage_opacity`
    /// on every `BOUNDARY_OPAQUE` transition; tombstoned on `BOUNDARY_TRANSPARENT`.
    /// Readers discard entries where `now_ms − written_at_ms` exceeds their evaporation window
    /// (no coordination needed). Graceful shutdown tombstones `sys/load/{node_id}/{kind}`
    /// automatically; callers may also call `agent.kv().delete(format!("sys/load/{}/{}", node_id, kind))`
    /// directly to force immediate evaporation.
    pub const LOAD:  &str = "sys/load/";
    /// Group membership namespace. Written automatically by `join_group`/`leave_group`.
    /// Key: `grp/<group_name>/<node_id>`. Value: `b"1"` (live) or tombstone (left).
    pub const GROUP: &str = "grp/";

    /// Advertised-capability namespace (optional persistence via
    /// `GossipAgent::advertise_persistent`).
    ///
    /// Key: `svc/{kind}/{node_id}`. Value: the payload bytes from the most recent
    /// advertise tick. Tombstoned automatically when the returned
    /// `AdvertiseHandle` is dropped or the agent shuts down.
    /// Late joiners can call `scan_prefix(kv_ns::ADVERTISE)` to find current capabilities
    /// without waiting for the next advertise tick.
    pub const ADVERTISE: &str = "svc/";

    /// **Reserved** for scoped mandates (v3 item 5 PR 1, `docs/design/scoped-mandates.md`).
    ///
    /// Key: `mandate/{scope}`. Value: `(holder, authority epoch)`.
    ///
    /// **A mandate here is an announcement, not the enforcement point.** The check that matters
    /// happens inside the protected resource's own atomic boundary — for `GitStore`, the mandate ref
    /// and the content ref move in one `update-ref` transaction. A mandate that lived only in KV
    /// would be a fact everyone agrees on and nothing enforces, which is the failure the record's
    /// decisive invariant is written against.
    pub const MANDATE: &str = "mandate/";

    /// **Reserved** for durable wiki proposals (v3 item 5 PR 1) — a stream under [`LOG`]'s
    /// `log/{stream}/{hlc_hex}` shape, written with the existing `KvHandle::append` verb.
    ///
    /// Key: `log/wiki/{group}/proposals`. Durable proposals use the log verb plus item 1's receipts
    /// rather than a service database; the *evaporating* KV queue stays what it always was — a
    /// delivery hint, not a record.
    pub const LOG_WIKI: &str = "log/wiki/";

    /// The commitment companion's prefix (v3 §6.9, `mycelium-commitment`): `cn/{requirement}` is the
    /// announcement head and `cn/{requirement}/award` the one award, written with a receipt; offers,
    /// reports and assessments are `append` streams under `log/cn/{requirement}/…`. The medium
    /// carries heads and terms only.
    pub const CN: &str = "cn/";

    /// **Reserved** (v3 item 4, `docs/design/adaptive-stability.md` §4, §11).
    ///
    /// Key: `rights/head/{holder}`. Value: a bounded, signed **head** — a holder's claim of the
    /// allocated rights it holds, checkable against the allocator's own journal. Heads only: the
    /// ledger is node-local and never enters the medium, because a right that evaporated with its
    /// holder's discovery entry would be issued twice. Nothing writes this prefix until item 4 PR 3.
    pub const RIGHTS_HEAD: &str = "rights/head/";

    /// **Reserved** for the knowledge layer (v3 item 3 PR 1, `docs/design/knowledge-layer.md`).
    ///
    /// Key: `knowledge/head/{issuer}/{stream}`. Value: a bounded, signed **discovery head** — a
    /// pointer, never a record. The records and their evidence live in an authorized store, which is
    /// the whole point: LWW may move a head, and moving a pointer cannot erase a competing
    /// statement. Equivocation is therefore preserved rather than HLC-resolved, and the layer can
    /// say *"these two issuers disagree"* instead of silently keeping the later one.
    ///
    /// Reserved at PR 1, before any code writes it, so the shape is settled while it is still free
    /// to settle — the alternative is discovering at PR 4 that something else took the prefix.
    pub const KNOWLEDGE_HEAD: &str = "knowledge/head/";

    /// Node identity namespace (library-internal — do not write from application code).
    ///
    /// Key: `sys/identity/{node_id}`. Value: 32-byte Ed25519 public key (raw bytes).
    /// Written at startup by nodes running with TLS enabled; used by peers to verify
    /// signed consensus messages when a mTLS cert extract is not yet available.
    pub const IDENTITY: &str = "sys/identity/";

    /// Signed-identity proof sibling of [`IDENTITY`] (identity-auth Phase 2): value is
    /// `signer_key(32) ‖ signature(64)` over the `sys/identity/{node}` history bytes. A new node
    /// accepts an identity entry into `peer_keys` only if a matching proof is chained to a key it
    /// already trusts for that node (CA anchor or a prior valid key); old nodes ignore this key.
    /// Deliberately not a `sys/identity/` sub-prefix so an `IDENTITY` prefix scan never sees it.
    pub const IDENTITY_PROOF: &str = "sys/identity-proof/";

    /// **The sealed identity record** (identity-auth Phase 3b) — key history *and* its proof in
    /// **one** KV entry, so they can never arrive apart. Key: `sys/identity-signed/{node_id}`.
    /// Value: `version(1) ‖ history ‖ proof(96)`, where `history` is the same key-history bytes
    /// [`IDENTITY`] carries and `proof` the same `signer(32) ‖ signature(64)` [`IDENTITY_PROOF`]
    /// carries, over those bytes.
    ///
    /// **Why it exists.** [`IDENTITY`] and [`IDENTITY_PROOF`] are two entries, hence two gossip
    /// messages with no ordering between them. A peer that requires proofs and learns the identity
    /// first rejects it and holds no key for that node until the proof lands — self-healing for the
    /// key, *not* self-healing for a leader election decided inside the window, which is one-shot.
    /// That window is why `require_identity_proofs` could not be turned on; one record closes it by
    /// construction rather than by timing.
    ///
    /// Every TLS node writes this **and** the legacy pair, so a node that predates this release
    /// still learns the key. Readers prefer this record; with `require_identity_proofs` set they
    /// accept **only** this record, because accepting the pair would reopen the window the flag
    /// exists to close. Deliberately not a `sys/identity/` sub-prefix, so an [`IDENTITY`] scan
    /// never sees it.
    pub const IDENTITY_SIGNED: &str = "sys/identity-signed/";

    /// **This node's acceptor record, so a restart cannot make it equivocate.**
    ///
    /// Key: `sys/consensus-accepted/{node_id}/{slot}`. Value since 2.30.0: `0x02 ‖ promised(8) ‖
    /// has_proposer(1) ‖ promised_to(8) ‖ has_accepted(1) ‖ accepted_ballot(8) ‖ value_digest(32) ‖
    /// value(optional)` — the **promise** as well as the acceptance (`mycelium::consensus`'s
    /// `encode_acceptor`). Before 2.30.0: `ballot(8, LE) ‖ value_digest(32)`, still read.
    ///
    /// A consensus acceptor's whole guarantee — *at most one value per ballot* — rests on
    /// remembering what it accepted. That memory was in-process, so a node that restarted mid-ballot
    /// forgot, and could vote again for a different value at the same ballot. This is the record
    /// that survives.
    ///
    /// The value is carried when it fits under the KV write cap: a node recovered from a record
    /// that has only the digest can **refuse** a conflicting vote — the safety property — but cannot
    /// hand the value to a proposer, which then cannot propose anything else for the slot.
    ///
    /// **Strictly self-owned**, like every other `sys/{…}/{self}` key: the value is this node's own
    /// testimony, and a peer's write to it under LWW would erase exactly the memory it exists to
    /// keep. **What it discloses:** the record gossips cluster-wide, and since 2.30.0 it carries an
    /// accepted value of up to 4 KiB — so a value that was proposed to a group but never committed
    /// is readable by nodes outside that group. Committed values already are (`consensus/committed/`).
    ///
    /// **Kept when the slot commits** (2.30.0). It used to be removed, and that dropped promises: a
    /// delayed lower-ballot proposal reaching acceptors that had forgotten them could commit a second
    /// value. The prefix grows with the number of slots, not of ballots.
    pub const CONSENSUS_ACCEPTED: &str = "sys/consensus-accepted/";

    /// Gateway caller-context marker (v3 item 7). Key: `sys/caller-context/{node}`, value: the
    /// envelope version this node enforces (`b"1"`). Written once at start by every node that
    /// strips and verifies the `GatewayCaller` envelope on its RPC receive path; a gateway in the
    /// secure profile dispatches to a provider **only** if the marker is present — a node without
    /// it is a pre-item-7 provider that would run the call as the gateway node, and the call is
    /// refused instead. Self-owned (`sys/` tripwire), never written for another node.
    pub const CALLER_CONTEXT: &str = "sys/caller-context/";

    /// Member removals (closure plan C5). Key: `sys/membership/removed/{node}`, value: the
    /// operator-signed `SignedMemberRemoval` as JSON. Written by the node that accepted it, so the
    /// removal spreads by gossip as well as by direct offer; every reader verifies it before
    /// applying it, so a forged entry does nothing.
    pub const MEMBERSHIP_REMOVED: &str = "sys/membership/removed/";

    /// Persistent quorum evidence namespace (library-internal — do not write from application code).
    ///
    /// Key: `sys/quorum/{kind}/{sender_node_id}`. Value: 8-byte little-endian Unix millisecond
    /// timestamp of when this node last received and admitted a signal of `kind` from
    /// `sender_node_id`. Written by the connection handler on every admitted signal
    /// delivery; anti-entropy synced to peers so the evidence survives process restarts.
    ///
    /// Use `GossipAgent::quorum_persistent` to query the count of distinct senders
    /// within a time window. Prefer `GossipAgent::quorum` (in-memory) for low-latency
    /// queries — `quorum_persistent` is only needed when quorum evidence must survive
    /// crashes or restarts.
    pub const QUORUM: &str = "sys/quorum/";

    /// Ordered durable log namespace.
    ///
    /// Key: `log/{stream}/{hlc:016x}`. The 16-char zero-padded hex HLC ensures
    /// lexicographic order equals time order. Written by `GossipAgent::append`;
    /// compacted by `GossipAgent::compact_log`.
    pub const LOG: &str = "log/";

    /// Consumer group offset cursors.
    ///
    /// Key: `clog/{stream}/{group}/offset`. Value: 16-char hex HLC of the last
    /// processed entry. Written by `GossipAgent::subscribe_log_group` after each
    /// entry is successfully delivered.
    pub const CONSUMER_LOG: &str = "clog/";

    /// Distributed lock state.
    ///
    /// Key: `lock/{name}`. Value: JSON `{"holder":"ip:port","token":u64,"expires_ms":u64}`.
    /// Written by `GossipAgent::distributed_lock`; tombstoned when the returned
    /// `LockGuard` is dropped.
    pub const LOCK: &str = "lock/";

    /// Prompt template namespace.
    ///
    /// Key: `prompts/{ns}/{name}`. Value: JSON-encoded `PromptTemplate`.
    /// TTL = 604800s (one week). Written once by `register_prompt_skill`; updated by
    /// `update_prompt`; tombstoned by `delete_prompt`. No refresh task — this is
    /// configuration, not a presence signal. The `cap/` entry is the presence heartbeat.
    pub const PROMPTS: &str = "prompts/";
}  // end kv_ns

// ── Reorder buffer ────────────────────────────────────────────────────────────

use std::collections::{BinaryHeap, HashMap as StdHashMap};
use std::cmp::Reverse;

/// The per-handler signal channel — a handler that cannot keep up drops signals.
const SIGNAL_CHAN: &str = "signal/handler";

/// Per-`(sender, kind)` min-heap entry, ordered by ascending `hlc_seq`.
struct PendingSignal {
    hlc_seq:     u64,
    signal:      Signal,
    received_at: Instant,
}

impl PartialEq  for PendingSignal { fn eq(&self, o: &Self) -> bool { self.hlc_seq == o.hlc_seq } }
impl Eq         for PendingSignal {}
impl PartialOrd for PendingSignal {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(o)) }
}
impl Ord for PendingSignal {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering { self.hlc_seq.cmp(&o.hlc_seq) }
}

/// Receiver-side causal delivery buffer for signals emitted via `emit_ordered`.
///
/// Signals are buffered per `(sender, kind)` in a min-heap keyed by `hlc_seq`.
/// Each call to `ingest` delivers everything in the heap in ascending HLC order,
/// discarding entries at or below the current watermark (already delivered).
/// `flush_expired` delivers signals that have been held longer than `max_hold`
/// or when a buffer exceeds `max_depth`, preventing head-of-line blocking.
pub struct SignalReorderBuffer {
    pending:    StdHashMap<(NodeId, Arc<str>), BinaryHeap<Reverse<PendingSignal>>>,
    watermarks: StdHashMap<(NodeId, Arc<str>), u64>,
    max_hold:   Duration,
    max_depth:  usize,
}

impl SignalReorderBuffer {
    pub fn new(max_hold: Duration, max_depth: usize) -> Self {
        Self {
            pending:    StdHashMap::new(),
            watermarks: StdHashMap::new(),
            max_hold,
            max_depth,
        }
    }

    /// Ingests a signal with its HLC emission timestamp. Returns signals ready
    /// to deliver in ascending HLC order. Signals at or below the watermark
    /// (already delivered) are silently discarded.
    pub fn ingest(&mut self, hlc_seq: u64, signal: Signal) -> Vec<Signal> {
        let key = (signal.sender.clone(), Arc::clone(&signal.kind));
        if hlc_seq <= self.watermarks.get(&key).copied().unwrap_or(0) {
            return vec![];
        }
        self.pending.entry(key.clone()).or_default()
            .push(Reverse(PendingSignal { hlc_seq, signal, received_at: Instant::now() }));
        self.drain(&key)
    }

    /// Delivers any signals older than `max_hold` or in buffers deeper than
    /// `max_depth`. Call on each receive-loop iteration to bound latency.
    pub fn flush_expired(&mut self) -> Vec<Signal> {
        let keys: Vec<_> = self.pending.keys().cloned().collect();
        let mut out = Vec::new();
        for key in keys { out.extend(self.drain(&key)); }
        out
    }

    // `drain` honors `max_hold`/`max_depth` — it must NOT unconditionally force-flush. `flush_expired`
    // used to call it with `force=true`, which drained the entire buffer on every receive-loop
    // iteration (it is called before each `ingest`), so nothing was ever held: a lower HLC seq
    // arriving after a higher one was delivered out of order and then dropped as stale — the exact
    // reordering the buffer exists to prevent (audit 2026-07-15 pass 4). The `force` param had no
    // other caller and is removed so it cannot be reintroduced.
    fn drain(&mut self, key: &(NodeId, Arc<str>)) -> Vec<Signal> {
        let Some(heap) = self.pending.get_mut(key) else { return vec![] };
        let wm = self.watermarks.entry(key.clone()).or_insert(0);
        let now = Instant::now();

        // Determine flush policy once, before any pops change the depth.
        let depth_overflow = heap.len() > self.max_depth;
        let flush = depth_overflow
            || heap.peek().is_some_and(|Reverse(t)| {
                crate::sim_seam::mono_span(&t.received_at, &now) >= self.max_hold
            });

        if !flush {
            return vec![];
        }

        if depth_overflow {
            tracing::warn!(
                sender = %key.0, kind = %key.1,
                depth = heap.len(), max_depth = self.max_depth,
                "signal reorder buffer exceeded max_depth; flushing early — \
                 causal ordering guarantee lost for this burst. \
                 Increase max_depth or reduce signal rate to avoid this.",
            );
        }

        let mut out = Vec::new();
        loop {
            match heap.peek() {
                None => break,
                Some(Reverse(top)) if top.hlc_seq <= *wm => { heap.pop(); } // stale
                _ => {
                    let Reverse(e) = heap.pop().expect("peek returned Some so pop cannot fail");
                    *wm = (*wm).max(e.hlc_seq);
                    out.push(e.signal);
                }
            }
        }

        if heap.is_empty() { self.pending.remove(key); }
        out
    }
}

#[cfg(test)]
mod seed_tests {
    use super::*;
    use crate::node_id::NodeId;
    use std::time::Duration;

    fn id(p: u16) -> NodeId { NodeId::new("127.0.0.1", p).unwrap() }

    /// **A seeded entry really is as old as it says it is.**
    ///
    /// `seed_sender_log` reconstructs a sender-log entry that arrived `age_ms` ago, from a
    /// `sys/quorum/` record, and `warm_quorum_from_layer1` calls it **at startup** — when the
    /// process is milliseconds old. Its age therefore has to point *before* the process began,
    /// which is why `sim_seam::MONO_ORIGIN_NS` is not zero.
    ///
    /// It had no test at all, in either representation. With a zero origin the reconstructed
    /// timestamp clamps to the run's start, every warmed record looks brand new, and a quorum built
    /// on stale evidence reads as live — in exactly the moment the mechanism exists for.
    #[test]
    fn a_seeded_entry_keeps_the_age_it_was_seeded_with() {
        let handlers = SignalHandlers::new(Duration::from_secs(600));
        let kind: Arc<str> = Arc::from("quorum.warm");
        handlers.seed_sender_log(Arc::clone(&kind), id(9101), 500);

        assert!(
            handlers.quorum(&kind, 1, Duration::from_secs(5)),
            "inside a 5 s window a 500 ms-old entry counts"
        );
        assert!(
            !handlers.quorum(&kind, 1, Duration::from_millis(200)),
            "but it is 500 ms old, so a 200 ms window must NOT count it — a clamped seed would"
        );
    }

    /// The guard that was already there: an entry older than the window is not seeded at all.
    #[test]
    fn an_entry_older_than_the_window_is_not_seeded() {
        let handlers = SignalHandlers::new(Duration::from_millis(100));
        let kind: Arc<str> = Arc::from("quorum.stale");
        handlers.seed_sender_log(Arc::clone(&kind), id(9102), 5_000);
        assert!(!handlers.quorum(&kind, 1, Duration::from_secs(600)), "never recorded");
    }
}

#[cfg(test)]
mod reorder_tests {
    use super::*;
    use crate::node_id::NodeId;
    use bytes::Bytes;
    use std::time::Duration;

    fn make_signal(sender: &NodeId, kind: &str, nonce: u64) -> Signal {
        Signal {
            kind:    Arc::from(kind),
            scope:   crate::signal::SignalScope::Cluster,
            payload: Bytes::new(),
            sender:  sender.clone(),
            nonce,
        }
    }

    fn node() -> NodeId { "127.0.0.1:9000".parse().unwrap() }

    #[test]
    fn inorder_delivered_at_zero_hold() {
        // max_hold=0ms → age >= 0ms always true → signals drain immediately from ingest.
        let mut buf = SignalReorderBuffer::new(Duration::from_millis(0), 64);
        let n = node();
        let out = buf.ingest(10, make_signal(&n, "test", 1));
        assert_eq!(out.len(), 1);
        let out = buf.ingest(20, make_signal(&n, "test", 2));
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn signals_held_until_max_hold_expires() {
        // With a large max_hold, signals are held in the buffer.
        let mut buf = SignalReorderBuffer::new(Duration::from_secs(60), 64);
        let n = node();
        let out = buf.ingest(10, make_signal(&n, "test", 1));
        assert!(out.is_empty(), "signal should be held");
        let out = buf.ingest(20, make_signal(&n, "test", 2));
        assert!(out.is_empty(), "signal should be held");
    }

    #[test]
    fn regression_flush_expired_holds_and_reorders_not_force_drains() {
        // Audit 2026-07-15 pass 4: the connection handler calls flush_expired() before EVERY ingest.
        // flush_expired used to force-drain the whole buffer, so a lower seq (10) arriving after a
        // higher one (20) was delivered out of order and then dropped as stale — the exact reordering
        // the buffer exists to prevent. Post-fix flush_expired honors max_hold, so the buffer holds
        // and reorders. (nonce == seq here.)
        let mut buf = SignalReorderBuffer::new(Duration::from_millis(50), 64);
        let n = node();
        let mut delivered: Vec<u64> = Vec::new();
        for seq in [20u64, 10, 30] {
            for s in buf.flush_expired() { delivered.push(s.nonce); }
            for s in buf.ingest(seq, make_signal(&n, "test", seq)) { delivered.push(s.nonce); }
        }
        // Within max_hold nothing is force-drained. Pre-fix this vec would already be [20, 30] with
        // seq=10 lost; post-fix it is empty (all three held pending reorder).
        assert!(delivered.is_empty(), "young signals must be held, not force-drained: {delivered:?}");
        // Once max_hold elapses, a single flush delivers ALL buffered signals in ascending HLC order.
        std::thread::sleep(Duration::from_millis(70));
        let ordered: Vec<u64> = buf.flush_expired().into_iter().map(|s| s.nonce).collect();
        assert_eq!(ordered, vec![10, 20, 30], "must deliver in ascending HLC order, none dropped");
    }

    #[test]
    fn stale_signal_discarded() {
        // max_hold=0ms so hlc_seq=20 is delivered immediately; hlc_seq=10 then stale.
        let mut buf = SignalReorderBuffer::new(Duration::from_millis(0), 64);
        let n = node();
        let out = buf.ingest(20, make_signal(&n, "test", 1));
        assert_eq!(out.len(), 1);
        // hlc_seq=10 arrives after hlc_seq=20 was delivered — stale
        let out = buf.ingest(10, make_signal(&n, "test", 2));
        assert!(out.is_empty());
    }

    #[test]
    fn max_depth_forces_flush_in_hlc_order() {
        // Signals arrive in reverse HLC order; third push exceeds max_depth=2 → flush all.
        let mut buf = SignalReorderBuffer::new(Duration::from_secs(999), 2);
        let n = node();
        let r1 = buf.ingest(30, make_signal(&n, "test", 1)); // depth=1, held
        let r2 = buf.ingest(20, make_signal(&n, "test", 2)); // depth=2, held
        let out = buf.ingest(10, make_signal(&n, "test", 3)); // depth=3>max → flush in HLC order
        assert!(r1.is_empty());
        assert!(r2.is_empty());
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].nonce, 3); // hlc_seq=10 (smallest)
        assert_eq!(out[1].nonce, 2); // hlc_seq=20
        assert_eq!(out[2].nonce, 1); // hlc_seq=30 (largest)
    }

    #[test]
    fn flush_expired_delivers_held_signals() {
        // flush_expired releases a signal only once it exceeds max_hold — it must NOT force-drain a
        // young one. (Before the pass-4 fix this test asserted the opposite, codifying the very
        // force-drain bug that broke ordered delivery — which is why the bug shipped green.)
        let mut buf = SignalReorderBuffer::new(Duration::from_millis(40), 64);
        let n = node();
        let out = buf.ingest(10, make_signal(&n, "test", 1));
        assert!(out.is_empty(), "signal should be held before flush");
        assert!(buf.flush_expired().is_empty(), "a young signal must stay held, not be force-drained");
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(buf.flush_expired().len(), 1, "a signal past max_hold must be delivered");
    }

    #[test]
    fn hlc_seq_none_bypasses_buffer() {
        // The buffer is only called when hlc_seq is Some — this test documents
        // that the None path doesn't go through the buffer at all (enforced by
        // the connection.rs call site, not by SignalReorderBuffer itself).
        let mut buf = SignalReorderBuffer::new(Duration::from_secs(1), 64);
        assert!(buf.pending.is_empty());
        let _ = buf.flush_expired(); // no-op on empty buffer
        assert!(buf.pending.is_empty());
    }
}

#[cfg(test)]
mod load_state_tests {
    use super::{decode_load_state, encode_load_state, LoadState};

    #[test]
    fn regression_decode_clamps_hostile_fill_ratio() {
        // Audit 2026-07-15 pass 5: fill_ratio is gossiped raw under sys/load/ (no write guard) and
        // multiplied downstream (the consensus retry-jitter sleep). decode must clamp it to [0,1]
        // (NaN → 0) so a poisoned 1e30/Inf/NaN cannot drive a ~584M-year Duration or a NaN.
        for (hostile, want) in [(1e30f32, 1.0f32), (f32::INFINITY, 1.0), (-5.0, 0.0), (0.5, 0.5)] {
            let enc = encode_load_state(&LoadState { fill_ratio: hostile, is_opaque: false, written_at_ms: 1 });
            let got = decode_load_state(&enc).expect("decode").fill_ratio;
            assert_eq!(got, want, "fill_ratio {hostile} must clamp to {want}");
        }
        // NaN clamps to 0 (not propagated).
        let enc = encode_load_state(&LoadState { fill_ratio: f32::NAN, is_opaque: false, written_at_ms: 1 });
        assert_eq!(decode_load_state(&enc).expect("decode").fill_ratio, 0.0, "NaN fill_ratio must decode to 0");
    }
}

#[cfg(test)]
mod bound_tests {
    //! Row B (post-360 hardening): the sender log is bounded per sender-chosen kind, and a full
    //! subscriber loses its own signals instead of vetoing its kind for everyone.
    use super::*;
    use crate::node_id::NodeId;
    use bytes::Bytes;
    use std::time::Duration;

    fn id(p: u16) -> NodeId { NodeId::new("127.0.0.1", p).unwrap() }

    fn sig(kind: &str, sender: NodeId) -> Signal {
        Signal {
            kind: Arc::from(kind), scope: SignalScope::Cluster,
            payload: Bytes::from_static(b"x"), sender, nonce: 0,
        }
    }

    #[test]
    fn the_signal_log_stays_bounded_under_random_kinds() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        for i in 0..SIGNAL_LOG_MAX_KINDS * 3 {
            h.deliver(&sig(&format!("rand.{i}"), id(1)));
        }
        assert!(
            h.log_kinds_tracked() <= SIGNAL_LOG_MAX_KINDS,
            "{} kinds tracked after {} random kinds; the bound is {SIGNAL_LOG_MAX_KINDS}",
            h.log_kinds_tracked(), SIGNAL_LOG_MAX_KINDS * 3,
        );
        assert!(h.log_kinds_evicted() >= (SIGNAL_LOG_MAX_KINDS * 2) as u64, "evictions are counted");
        assert_eq!(h.log_kinds_refused(), 0, "nothing is exempt, so nothing is refused");
    }

    #[test]
    fn one_kind_stays_bounded_under_random_senders() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        for p in 0..(SIGNAL_LOG_MAX_SENDERS_PER_KIND * 3) as u16 {
            h.deliver(&sig("one.kind", id(1 + p)));
        }
        assert!(
            h.log_entries("one.kind") <= SIGNAL_LOG_MAX_SENDERS_PER_KIND,
            "{} entries for one kind; the bound is {SIGNAL_LOG_MAX_SENDERS_PER_KIND}",
            h.log_entries("one.kind"),
        );
    }

    /// The per-kind bound must not cost a quorum its answer: a sender repeating itself keeps one
    /// entry, so it cannot push a quieter sender out.
    #[test]
    fn a_repeating_sender_does_not_push_a_quieter_one_out() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        h.deliver(&sig("hb", id(1)));
        for _ in 0..SIGNAL_LOG_MAX_SENDERS_PER_KIND * 4 {
            h.deliver(&sig("hb", id(2)));
        }
        assert!(h.quorum("hb", 2, Duration::from_secs(60)), "both senders still count");
        assert!(h.log_entries("hb") <= 2, "one entry per sender");
    }

    /// A kind this node subscribes to is tracked even when random kinds have filled the log.
    #[test]
    fn a_subscribed_kind_is_tracked_when_the_log_is_full() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        for i in 0..SIGNAL_LOG_MAX_KINDS + 10 {
            h.deliver(&sig(&format!("rand.{i}"), id(1)));
        }
        let _rx = h.register(Arc::from("mine"));
        h.deliver(&sig("mine", id(7)));
        assert!(h.quorum("mine", 1, Duration::from_secs(60)));
        assert!(h.last_signal("mine").is_some());
    }

    /// Finding (25): `fill_ratio` was the MAX over open subscribers, so one subscriber that stopped
    /// reading drove its kind's fill to 1.0 and the boundary admitted nothing of that kind — for
    /// every other subscriber on the node.
    #[test]
    fn a_stalled_subscriber_does_not_veto_its_kind() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        let kind: Arc<str> = Arc::from("busy");
        let _stalled = h.register(Arc::clone(&kind));
        let mut reading = h.register(Arc::clone(&kind));
        for _ in 0..300 {
            h.deliver(&sig("busy", id(1)));
            while reading.try_recv().is_ok() {}
        }
        assert!(
            h.fill_ratio(&kind) < 0.5,
            "a reading subscriber has room, so the kind is not overloaded; fill = {}",
            h.fill_ratio(&kind),
        );
        assert!(h.handler_drops() >= 300 - 256, "the stalled subscriber's losses are counted");
    }

    /// Genuine overload still sheds: when every subscriber of a kind is full, the kind is full.
    #[test]
    fn a_kind_whose_only_subscriber_is_full_still_sheds() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        let kind: Arc<str> = Arc::from("solo");
        let _rx = h.register_with_capacity(Arc::clone(&kind), 4);
        for _ in 0..4 {
            h.deliver(&sig("solo", id(1)));
        }
        assert_eq!(h.fill_ratio(&kind), 1.0);
    }

    /// #602's review, finding 2: `quorum_written` was the sender log's unbounded twin — one entry per
    /// `(kind, sender)` ever seen, and a `sys/quorum/` KV write for every kind, tracked or not.
    #[test]
    fn quorum_evidence_stays_bounded_with_the_log() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        for i in 0..SIGNAL_LOG_MAX_KINDS * 3 {
            let k: Arc<str> = Arc::from(format!("rand.{i}"));
            h.deliver(&sig(&k, id(1)));
            let _ = h.quorum_evidence_payload(&k, &id(1));
        }
        assert!(h.evidence_entries() <= SIGNAL_LOG_MAX_KINDS,
                "{} evidence entries after {} random kinds", h.evidence_entries(), SIGNAL_LOG_MAX_KINDS * 3);
    }

    /// Finding 2, second half: a kind the log could not track writes no KV evidence.
    #[test]
    fn a_kind_the_log_refuses_writes_no_evidence() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        let mut keep = Vec::new();
        for i in 0..SIGNAL_LOG_MAX_KINDS {
            let k: Arc<str> = Arc::from(format!("sub.{i}"));
            keep.push(h.register(Arc::clone(&k))); // every tracked kind is subscribed: none evictable
            h.deliver(&sig(&k, id(1)));
        }
        let stray: Arc<str> = Arc::from("stray");
        h.deliver(&sig(&stray, id(2)));
        assert!(h.quorum_evidence_payload(&stray, &id(2)).is_none());
    }

    /// Finding 3: refusing new kinds at the cap turned the memory bound into a false negative — a
    /// kind first seen after 4096 random ones could never reach a quorum.
    #[test]
    fn a_kind_first_seen_after_a_flood_is_still_tracked() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        for i in 0..SIGNAL_LOG_MAX_KINDS + 10 {
            h.deliver(&sig(&format!("rand.{i}"), id(1)));
        }
        h.deliver(&sig("late.kind", id(2)));
        assert!(h.quorum("late.kind", 1, Duration::from_secs(60)), "least-recently-seen kinds make room");
        assert!(h.log_kinds_tracked() <= SIGNAL_LOG_MAX_KINDS);
    }

    /// Finding 3, the exemption: a kind someone queries is not evicted by a flood.
    #[test]
    fn a_queried_kind_survives_a_flood() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        h.deliver(&sig("watched", id(1)));
        assert!(h.quorum("watched", 1, Duration::from_secs(60)));
        for i in 0..SIGNAL_LOG_MAX_KINDS * 3 {
            h.deliver(&sig(&format!("rand.{i}"), id(2)));
        }
        assert!(h.quorum("watched", 1, Duration::from_secs(60)), "a queried kind is exempt from eviction");
        assert!(h.last_signal("watched").is_some());
    }

    /// Finding 3, senders: an active sender stays; forged ones heard from longer ago go first.
    #[test]
    fn an_active_sender_outlives_older_forged_ones() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        for p in 0..SIGNAL_LOG_MAX_SENDERS_PER_KIND as u16 {
            h.deliver(&sig("k", id(10_000 + p)));
        }
        h.deliver(&sig("k", id(1)));
        for p in 0..200u16 {
            h.deliver(&sig("k", id(20_000 + p)));
        }
        assert!(h.log_entries("k") <= SIGNAL_LOG_MAX_SENDERS_PER_KIND);
        let members: AHashSet<u64> = [id(1).id_hash()].into_iter().collect();
        assert!(h.quorum_for_group("k", &members, 1, Duration::from_secs(60)));
    }

    /// Finding 4: under the least-full rule a fast passive tap hid a saturated worker, so the kind
    /// never shed. Taps do not count toward fill.
    #[test]
    fn a_fast_tap_does_not_hide_a_saturated_worker() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        let kind: Arc<str> = Arc::from("work");
        let _worker = h.register_with_capacity(Arc::clone(&kind), 4);
        let mut tap = h.register_tap(Arc::clone(&kind), 256);
        for _ in 0..4 {
            h.deliver(&sig("work", id(1)));
            while tap.try_recv().is_ok() {}
        }
        assert_eq!(h.fill_ratio(&kind), 1.0, "the only worker is full: the kind is full");
    }

    /// A kind with only a tap has no work to shed.
    #[test]
    fn a_kind_with_only_a_full_tap_is_not_full() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        let kind: Arc<str> = Arc::from("watch.only");
        let _tap = h.register_tap(Arc::clone(&kind), 2);
        for _ in 0..4 {
            h.deliver(&sig("watch.only", id(1)));
        }
        assert_eq!(h.fill_ratio(&kind), 0.0);
        assert!(h.handler_drops() >= 2, "the tap's own losses are counted");
    }

    /// #602's re-review, finding 1: evicting instead of refusing meant every flooded kind still wrote
    /// a `sys/quorum/` key — appended to the WAL, gossiped, never collected.
    #[test]
    fn a_random_kind_flood_writes_no_evidence() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        let mut written = 0;
        for i in 0..2_000 {
            let k: Arc<str> = Arc::from(format!("rand.{i}"));
            h.deliver(&sig(&k, id(1)));
            written += usize::from(h.quorum_evidence_payload(&k, &id(1)).is_some());
        }
        assert_eq!(written, 0, "a kind nobody here works on or asks about writes no KV evidence");
    }

    /// Finding 1, second half: the rate-limit table at its cap suppressed evidence for a kind this
    /// node works on — a false negative for `quorum_persistent`.
    #[test]
    fn a_flood_does_not_suppress_a_worked_kinds_evidence() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        let _worker = h.register(Arc::from("legit"));
        for i in 0..SIGNAL_LOG_MAX_KINDS * 2 {
            let k: Arc<str> = Arc::from(format!("rand.{i}"));
            h.deliver(&sig(&k, id(1)));
            let _ = h.quorum_evidence_payload(&k, &id(1));
        }
        let legit: Arc<str> = Arc::from("legit");
        h.deliver(&sig("legit", id(2)));
        assert!(h.quorum_evidence_payload(&legit, &id(2)).is_some());
    }

    /// Finding 4: an SSE tap made its kind exempt from eviction, so `mesh:read` streams could pin kinds.
    #[test]
    fn a_tap_does_not_exempt_its_kind_from_eviction() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        let _tap = h.register_tap(Arc::from("tapped"), 8);
        h.deliver(&sig("tapped", id(1)));
        for i in 0..SIGNAL_LOG_MAX_KINDS * 2 {
            h.deliver(&sig(&format!("rand.{i}"), id(1)));
        }
        assert_eq!(h.log_entries("tapped"), 0, "a tap is an observer; its kind is evictable");
    }

    /// Finding 5: a query pinned its kind for ever; after 4096 distinct queries nothing more could be
    /// pinned. A pin lasts one window from the last query.
    #[test]
    fn a_query_pin_expires() {
        let h = SignalHandlers::new(Duration::from_millis(100));
        h.deliver(&sig("asked", id(1)));
        assert!(h.quorum("asked", 1, Duration::from_secs(60)));
        std::thread::sleep(Duration::from_millis(250));
        for i in 0..SIGNAL_LOG_MAX_KINDS * 2 {
            h.deliver(&sig(&format!("rand.{i}"), id(1)));
        }
        assert_eq!(h.log_entries("asked"), 0, "a pin not renewed within the window lapses");
    }

    /// Finding 6: with most kinds exempt, every new kind scanned the whole table.
    #[test]
    fn eviction_scans_are_amortised_when_most_kinds_are_exempt() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        let mut keep = Vec::new();
        for i in 0..SIGNAL_LOG_MAX_KINDS {
            let k: Arc<str> = Arc::from(format!("sub.{i}"));
            keep.push(h.register(Arc::clone(&k)));
            h.deliver(&sig(&k, id(1)));
        }
        for i in 0..2_000 {
            h.deliver(&sig(&format!("rand.{i}"), id(1)));
        }
        assert!(h.log_eviction_scans() <= 2_000 / (SIGNAL_LOG_MAX_KINDS as u64 / 8) as usize as u64 + 2,
                "{} scans for 2000 new kinds", h.log_eviction_scans());
    }

    /// #602's round 3, finding 2: a random-sender flood on a kind this node works on wrote a
    /// `sys/quorum/` key per forged sender at line rate — at the table's cap the write went ahead
    /// unremembered. Evidence is bounded per kind by the log's sender cap, and the table bound holds.
    #[test]
    fn evidence_writes_are_bounded_per_kind_and_by_the_table() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        let mut keep = Vec::new();
        let mut written = 0usize;
        let started = std::time::Instant::now();
        for k in 0..8 {
            let kind: Arc<str> = Arc::from(format!("worked.{k}"));
            keep.push(h.register(Arc::clone(&kind)));
            for p in 0..(SIGNAL_LOG_MAX_SENDERS_PER_KIND as u16 + 500) {
                let sender = id(1 + p);
                h.deliver(&sig(&kind, sender.clone()));
                written += usize::from(h.quorum_evidence_payload(&kind, &sender).is_some());
            }
        }
        // The table bounds writes to its size per second (entries older than a second are reusable);
        // the per-kind bound to 8 × the sender cap in all.
        let seconds = started.elapsed().as_secs() as usize + 1;
        assert!(written <= SIGNAL_LOG_MAX_KINDS * seconds,
                "{written} evidence writes in {seconds} s; the table bounds them at {SIGNAL_LOG_MAX_KINDS} a second");
        assert!(written <= 8 * SIGNAL_LOG_MAX_SENDERS_PER_KIND);
        assert!(h.evidence_entries() <= SIGNAL_LOG_MAX_KINDS);
    }

    #[test]
    fn one_kinds_evidence_is_bounded_by_the_sender_cap() {
        // Round 3 bounded the keys a kind ever writes; round 4 made the set slide, so what stays
        // bounded is the set (memory) and — through the table — the write rate.
        let h = SignalHandlers::new(Duration::from_secs(600));
        let kind: Arc<str> = Arc::from("worked");
        let _w = h.register(Arc::clone(&kind));
        let started = std::time::Instant::now();
        let mut written = 0usize;
        for p in 0..(SIGNAL_LOG_MAX_SENDERS_PER_KIND as u16 * 3) {
            let sender = id(1 + p);
            h.deliver(&sig(&kind, sender.clone()));
            written += usize::from(h.quorum_evidence_payload(&kind, &sender).is_some());
        }
        assert!(h.evidence_senders("worked") <= SIGNAL_LOG_MAX_SENDERS_PER_KIND, "{} senders held", h.evidence_senders("worked"));
        let seconds = started.elapsed().as_secs() as usize + 1;
        assert!(written <= SIGNAL_LOG_MAX_KINDS * seconds, "{written} writes in {seconds} s");
    }


    /// #602's round 4, finding 3: a kind's evidence sender set only grew — after 1024 distinct senders,
    /// ever, the kind wrote no more evidence until a restart. The set slides: the sender written longest
    /// ago gives way.
    #[test]
    fn an_evidence_sender_set_slides() {
        let h = SignalHandlers::new(Duration::from_secs(600));
        let kind: Arc<str> = Arc::from("worked");
        let _w = h.register(Arc::clone(&kind));
        for p in 0..SIGNAL_LOG_MAX_SENDERS_PER_KIND as u16 {
            let sender = id(1 + p);
            h.deliver(&sig(&kind, sender.clone()));
            assert!(h.quorum_evidence_payload(&kind, &sender).is_some());
        }
        let late = id(30_000);
        h.deliver(&sig(&kind, late.clone()));
        assert!(h.quorum_evidence_payload(&kind, &late).is_some(), "a new sender past the cap displaces the oldest");
    }

    /// Finding 3: the outer map grew with every kind ever pinned. A kind whose pin lapsed and that has
    /// no worker loses its sender set at the next trim.
    #[test]
    fn a_lapsed_kinds_evidence_set_is_pruned() {
        let h = SignalHandlers::new(Duration::from_millis(100));
        let kind: Arc<str> = Arc::from("asked");
        assert!(!h.quorum("asked", 1, Duration::from_secs(60))); // pins it
        h.deliver(&sig("asked", id(1)));
        assert!(h.quorum_evidence_payload(&kind, &id(1)).is_some());
        assert_eq!(h.evidence_kinds(), 1);
        std::thread::sleep(Duration::from_millis(250));
        h.trim_sender_log(Duration::from_millis(100));
        assert_eq!(h.evidence_kinds(), 0, "no worker and a lapsed pin: the set goes");
    }
}
