//! # mycelium-blackboard — shared working memory on Mycelium's public API
//!
//! Blackboard-style **opportunistic multi-agent reasoning over typed facts**, rebuilt on Mycelium
//! the same way [`mycelium-tuple-space`](https://docs.rs/mycelium-tuple-space) rebuilt work
//! distribution. The design rationale + worked example (a community microgrid) live in
//! `docs/plans/mycelium-blackboard.md`; the phased build plan in
//! `docs/plans/v2-wsg-g3-blackboard.md`.
//!
//! ## The reading / consuming split (Linda's `rd` vs `in`)
//!
//! A blackboard surfaces one clean distinction:
//!
//! - **Reading facts is unconditional and concurrent** (`rd`). Many agents observe the same fact —
//!   a forecaster and a pricing agent both react to a surplus-energy fact. Mycelium's substrate
//!   already does this perfectly via gossiped KV + boundary predicates; nothing new is needed.
//! - **Consuming facts is competitive and exactly-once** (`in`). Acting on a *finite* fact (the
//!   surplus exists once) means two agents whose triggers both match must **race for an atomic
//!   claim** — exactly one consumes it, the loser's claim returns empty, and a winner that drops
//!   mid-work has the claim re-queued.
//!
//! This crate adds the **one** missing primitive: **competitive destructive claim-by-predicate**.
//! Everything else (fact propagation, trigger predicates, evaporation) is the substrate's `rd`.
//!
//! ## Why not the tuple space
//!
//! The tuple space routes by *position* (named FIFO lanes, topology known per stage). The blackboard
//! routes by *content*: a consumer's criterion is a **predicate over fact attributes**, and the
//! topology is *emergent per item* — a surplus fact routes through entirely different agents than a
//! deficit fact. A lane per (fact-type × interest) explodes against each agent's private, changing
//! declarations. The predicate language is the **capability attribute-filter grammar** (equality +
//! presence), *not* unification — already implemented, already understood, and enough for trigger
//! conditions.
//!
//! ## Status (WS-G / G3)
//!
//! - **[`BoardStore`]** — the pure claim-by-predicate core (`post` / `read` / `claim` / `ack` /
//!   `release`), single-owner and exactly-once, testable without a cluster, with WAL durability.
//! - **[`Blackboard`]** — the agent-backed board: posting/reading/claiming over a coordinator-free
//!   primary discovered on the capability ring, with emergent secondary failover (`Post`/`Ack`
//!   replication + snapshot sync + promotion-on-evaporation).
//!
//! Remaining phases add the HTTP gateway + SDKs and the worked example.

#![deny(unsafe_code)]

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use mycelium::{CapFilter, Capability, CapabilityReg, GossipAgent, NodeId};

#[cfg(feature = "gateway")]
mod http;
mod rpc;
mod store;
mod wal;
pub use store::{BoardDepth, BoardStats, BoardStore};

/// A typed fact on the board: an attribute map (the matchable surface) plus an opaque payload.
/// Facts are non-destructively *read* by many and destructively *claimed* by at most one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    /// Board-assigned id; also the claim handle once the fact is claimed.
    pub id: u64,
    /// The matchable surface — string attributes a [`Predicate`] tests (the content-plane analogue
    /// of a capability's attributes). Routing is encoded here, never in the payload.
    pub attributes: BTreeMap<String, String>,
    /// Opaque payload (the substrate never matches on it).
    pub payload: bytes::Bytes,
}

/// One attribute constraint in a [`Predicate`]. Deliberately the capability attribute-filter
/// grammar — equality + presence — **not** unification/structural matching (scope creep until
/// demonstrated; see the crate docs' non-goals).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttrMatch {
    /// The attribute must be present and equal to this value.
    Equals(String),
    /// The attribute must be present (any value).
    Present,
}

/// A conjunctive predicate over fact attributes: **all** constraints must hold for a fact to match.
/// An empty predicate matches every fact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Predicate {
    attrs: BTreeMap<String, AttrMatch>,
}

impl Predicate {
    /// A predicate matching every fact. Add constraints with [`eq`](Self::eq) / [`present`](Self::present).
    pub fn new() -> Self {
        Self { attrs: BTreeMap::new() }
    }

    /// Require attribute `key` to equal `value`.
    pub fn eq(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attrs.insert(key.into(), AttrMatch::Equals(value.into()));
        self
    }

    /// Require attribute `key` to be present (any value).
    pub fn present(mut self, key: impl Into<String>) -> Self {
        self.attrs.insert(key.into(), AttrMatch::Present);
        self
    }

    /// True iff every constraint holds against `attributes`.
    pub fn matches(&self, attributes: &BTreeMap<String, String>) -> bool {
        self.attrs.iter().all(|(k, m)| match m {
            AttrMatch::Present => attributes.contains_key(k),
            AttrMatch::Equals(v) => attributes.get(k).is_some_and(|got| got == v),
        })
    }

    /// Number of constraints (0 = match-all).
    pub fn len(&self) -> usize {
        self.attrs.len()
    }

    /// Whether this is the match-all predicate.
    pub fn is_empty(&self) -> bool {
        self.attrs.is_empty()
    }
}

/// Errors from the board API.
#[derive(Debug)]
#[non_exhaustive]
pub enum BlackboardError {
    /// Unknown claim id — already acked, already released, re-queued by the in-flight deadline, or
    /// never claimed.
    NotFound,
    /// No node currently serves this board (later phases — role resolution).
    NoProvider,
    /// A `post` refused at admission: the claimable pool stood at the configured
    /// `high_watermark` (item 4 PR 5). Counted in `BoardStats::rejected`; a visible outcome.
    Backpressure { available: u64, high_watermark: u64 },
    /// Transport error talking to the board primary (later phases).
    Rpc(String),
    /// WAL I/O error (persistent boards).
    Io(std::io::Error),
    /// Refused at the claim door (adopter-surface Y11): an exposure rule selected the claim, and the action evaluator
    /// or the exposure gate refused it. Nothing was claimed. Only [`claim_in`](Blackboard::claim_in) returns it.
    Refused(mycelium::door::DoorRefusal),
}

impl std::fmt::Display for BlackboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlackboardError::NotFound => write!(f, "unknown claim id"),
            BlackboardError::NoProvider => write!(f, "no blackboard primary resolvable"),
            BlackboardError::Backpressure { available, high_watermark } => write!(
                f,
                "post refused at admission: {available} facts available, high watermark {high_watermark}"
            ),
            BlackboardError::Rpc(s) => write!(f, "rpc error: {s}"),
            BlackboardError::Io(e) => write!(f, "wal io error: {e}"),
            BlackboardError::Refused(r) => r.fmt(f),
        }
    }
}

/// The resource a claim door names: `bb:{ns}` (a predicate names no single resource).
pub(crate) fn claim_resource(ns: &str) -> String {
    format!("bb:{ns}")
}

/// What a claim did, for the door's record: a fact, nothing, or unknown (a transport error — the primary may have
/// handed one out).
pub(crate) fn claim_outcome(claimed: &Result<Option<Fact>, BlackboardError>) -> mycelium::door::DoorOutcome {
    use mycelium::door::DoorOutcome;
    match claimed {
        Ok(Some(_)) => DoorOutcome::Completed,
        Err(BlackboardError::Rpc(_)) => DoorOutcome::Unknown,
        _ => DoorOutcome::Nothing,
    }
}

impl std::error::Error for BlackboardError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BlackboardError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for BlackboardError {
    fn from(e: std::io::Error) -> Self {
        BlackboardError::Io(e)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Agent-backed board (WS-G / G3 · Phase 3) — emergent roles + failover.
//
// Mirrors the mycelium-tuple-space role pattern: the primary is discovered by capability
// advertisement (`blackboard/{ns}.primary`); a secondary mirrors via Post/Ack replication + an
// initial snapshot and promotes when the primary's capability evaporates (the ring IS the failure
// detector); `Auto` self-elects under the ring's negotiated election rule. No coordinator assigns roles.
// ═══════════════════════════════════════════════════════════════════════════════

/// Role this node plays for a board namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardRole {
    /// Advertise as candidate, settle, then become primary (the ring's negotiated election rule —
    /// `mycelium::election`) or secondary.
    Auto,
    /// Serve the board immediately.
    Primary,
    /// Mirror the primary and promote when its advertisement evaporates.
    Secondary,
    /// Pure poster/claimer; never serves.
    Client,
}

/// Board configuration.
#[derive(Debug, Clone)]
pub struct BoardConfig {
    /// Namespace, e.g. `"microgrid"`. Must not contain `/`. Advertised as capability
    /// `blackboard`/`{ns}.primary`; RPC kinds are `blackboard.{ns}.claim` etc.
    pub namespace: Arc<str>,
    pub role: BoardRole,
    /// WAL-backed (`true`) or transient (`false`).
    pub persist: bool,
    /// Ignored when `persist` is `false`.
    pub wal_path: PathBuf,
    /// Appends between `fdatasync` calls.
    pub checkpoint_every: u64,
    /// In-flight claim deadline: a claim not acked within this window re-queues the fact
    /// (at-least-once — the "claimer dropped mid-work" path).
    pub claim_timeout_secs: u64,
    /// Capability advertisement refresh. Readers evaporate ads at 3×, so promotion latency after a
    /// primary crash is ≈3× this value.
    pub cap_refresh: Duration,
    /// Admission bound on the claimable pool (item 4 PR 5): a `post` that would take `available`
    /// to or past this is refused with [`BlackboardError::Backpressure`] and counted in
    /// [`BoardStats::rejected`]. `None` = unbounded, the behaviour before v2.8.0. Applies at
    /// admission only — replication and WAL replay never refuse. A self-imposed bound (Tier B),
    /// not a hard one: a burst can briefly overshoot between the check and the insert.
    pub high_watermark: Option<u64>,
}

impl Default for BoardConfig {
    fn default() -> Self {
        Self {
            namespace: Arc::from("board"),
            role: BoardRole::Auto,
            persist: false,
            wal_path: PathBuf::from("board.wal"),
            checkpoint_every: 500,
            claim_timeout_secs: 300,
            cap_refresh: Duration::from_secs(10),
            high_watermark: None,
        }
    }
}

/// Mirror sync-loop ticks (each `cap_refresh`) between digest probes (ER6 PR B).
const DIGEST_PROBE_TICKS: u32 = 3;

/// Consecutive mismatched probes that start a full sync (review I3), whatever the primary's write
/// rate: a lost `Post` or `Ack` is repaired within this many probes — 4 × 3 × `cap_refresh`, two
/// minutes at the default 10 s, plus up to one probe interval before the first mismatch is seen.
/// A sync that repairs nothing (replication was only in flight) doubles the threshold, up to
/// `DIGEST_MISMATCHES_MAX`; a sync that repairs something resets it (review round 2).
const DIGEST_MISMATCHES_TO_SYNC: u32 = 4;

/// The backed-off threshold's ceiling: 32 × 3 × `cap_refresh`, 16 minutes at the default — the
/// worst-case repair time while it holds, and the interval between needless full syncs on a board
/// whose replication is always in flight at probe time.
const DIGEST_MISMATCHES_MAX: u32 = 32;

/// After a primary answers a probe with a full snapshot (older than PR B), probe once in this many
/// (review N1) — each such answer is the whole board.
const LEGACY_PROBE_BACKOFF: u32 = 10;

/// What one digest probe found (ER6 PR B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Probe {
    /// The mirror's vouched facts (with its refused-but-live ids) match the primary's live set.
    Match,
    /// They differ; the primary's next id at this probe (0 without a trailer).
    Mismatch { next_id: u64 },
    /// The primary is older than PR B: its answer was a full snapshot, applied.
    Legacy,
    /// No answer, or no store.
    Unknown,
}

/// An agent-backed board: posting/reading/claiming over a coordinator-free primary discovered on the
/// capability ring, with emergent secondary failover. Construct after `agent.start()`.
pub struct Blackboard {
    agent: Arc<GossipAgent>,
    cfg: BoardConfig,
    store: parking_lot::Mutex<Option<Arc<BoardStore>>>,
    is_primary: AtomicBool,
    is_secondary: AtomicBool,
    primary_reg: parking_lot::Mutex<Option<CapabilityReg>>,
    role_reg: parking_lot::Mutex<Option<CapabilityReg>>,
    /// The primary this mirror last heard from (a replicate, a snapshot), for the promotion-time sync
    /// when a refused fact is pending.
    last_primary: parking_lot::Mutex<Option<NodeId>>,
    /// Set by `shutdown` before it drains the tasks, so an `init_store` racing it aborts the
    /// maintenance task it starts instead of leaving it to hold the store (and the WAL's lock).
    shutting_down: AtomicBool,
    /// Set when a mirrored fact could not be applied (its WAL append was refused): the mirror re-syncs
    /// from the primary, so the fact is re-delivered once the WAL is repaired.
    mirror_gap: AtomicBool,
    /// Mirrored facts a primary's snapshot named acked and this mirror then discarded (ER6).
    mirror_retired: AtomicU64,
    /// Ids the primary's last full snapshot carried live that this mirror refused, remembering
    /// them acked (review I1: an ack lost before an old primary died). Folded into the mirror's
    /// digest, so a refusal no sync can cure does not start a sync every few probes.
    refused_live: parking_lot::Mutex<HashSet<u64>>,
    /// Live facts this mirror holds that the primary's last full snapshot neither carried nor named
    /// acked (ER6, D6c): kept, counted, and left out of the digest. Replaced at each full sync.
    unvouched: parking_lot::Mutex<HashSet<u64>>,
    /// Full syncs a digest mismatch started (ER6 PR B).
    digest_resyncs: AtomicU64,
    /// Test seam: hold each replicated record this long before sending it — replication always in
    /// flight at probe time, with nothing lost.
    #[cfg(test)]
    replicate_delay_ms: AtomicU64,
    tasks: parking_lot::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Blackboard {
    /// Construct the board and start whatever machinery the configured role needs.
    pub async fn new(agent: Arc<GossipAgent>, cfg: BoardConfig) -> Result<Arc<Self>, BlackboardError> {
        let bb = Arc::new(Self {
            agent,
            cfg,
            store: parking_lot::Mutex::new(None),
            is_primary: AtomicBool::new(false),
            is_secondary: AtomicBool::new(false),
            primary_reg: parking_lot::Mutex::new(None),
            role_reg: parking_lot::Mutex::new(None),
            last_primary: parking_lot::Mutex::new(None),
            shutting_down: AtomicBool::new(false),
            mirror_gap: AtomicBool::new(false),
            mirror_retired: AtomicU64::new(0),
            refused_live: parking_lot::Mutex::new(HashSet::new()),
            unvouched: parking_lot::Mutex::new(HashSet::new()),
            digest_resyncs: AtomicU64::new(0),
            #[cfg(test)]
            replicate_delay_ms: AtomicU64::new(0),
            tasks: parking_lot::Mutex::new(Vec::new()),
        });
        match bb.cfg.role {
            BoardRole::Primary => {
                bb.init_store()?;
                bb.become_primary();
            }
            BoardRole::Secondary => {
                bb.init_store()?;
                bb.become_secondary();
            }
            BoardRole::Auto => {
                let me = Arc::clone(&bb);
                let h = tokio::spawn(async move { me.run_election().await });
                bb.tasks.lock().push(h);
            }
            BoardRole::Client => {}
        }
        // Adopter-surface Y11 (#708's review, finding 1): a raw gateway call of `blackboard.{ns}.claim` — `rpc/call`,
        // `scatter`, `overlay/emit_reliable` — runs the claim door's decision on `bb:{ns}`, as the claim door does —
        // at a gateway node that hosts this board; a gateway without it passes the call undecided.
        let resource = claim_resource(&bb.cfg.namespace);
        bb.agent.register_door_kind(rpc::rpc_kind(&bb.cfg.namespace, "claim"), mycelium::door::OP_BB_CLAIM, Arc::new(move |_: &[u8]| resource.clone()));
        Ok(bb)
    }

    fn init_store(&self) -> Result<(), BlackboardError> {
        let created = {
            let mut g = self.store.lock();
            if g.is_some() {
                return Ok(());
            }
            let store = Arc::new(
                if self.cfg.persist {
                    BoardStore::persistent(&self.cfg.wal_path, self.cfg.checkpoint_every)?
                } else {
                    BoardStore::transient()
                }
                .with_high_watermark(self.cfg.high_watermark),
            );
            *g = Some(Arc::clone(&store));
            store
        };
        // WAL maintenance runs wherever a WAL does — a mirror's too (#597 review, finding 1): the
        // compaction that repairs a writer poisoned by a failed append, and the periodic sync. It ran
        // on the primary only, so a secondary's poisoned WAL refused every later mirrored fact.
        if self.cfg.persist {
            let h = tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(1));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    let s = Arc::clone(&created);
                    let _ = tokio::task::spawn_blocking(move || s.maintenance_tick()).await;
                }
            });
            let mut tasks = self.tasks.lock();
            if self.shutting_down.load(Ordering::SeqCst) {
                h.abort(); // `shutdown` already drained the tasks; nothing else would stop this one
            } else {
                tasks.push(h);
            }
        }
        Ok(())
    }

    pub(crate) fn store(&self) -> Option<Arc<BoardStore>> {
        self.store.lock().clone()
    }
    pub(crate) fn cfg(&self) -> &BoardConfig {
        &self.cfg
    }
    pub(crate) fn agent(&self) -> &Arc<GossipAgent> {
        &self.agent
    }
    /// This board's namespace.
    pub fn namespace(&self) -> &Arc<str> {
        &self.cfg.namespace
    }
    fn store_expect(&self) -> Arc<BoardStore> {
        self.store().expect("serving role requires a store")
    }
    /// Mirrored facts this node discarded because a primary's snapshot named them acked (ER6):
    /// facts completed while this mirror was away, or whose replicated `Ack` was lost.
    pub fn mirror_retired(&self) -> u64 {
        self.mirror_retired.load(Ordering::Relaxed)
    }

    /// Live facts this mirror holds that the primary's last full snapshot could not vouch for
    /// (ER6, D6c) — neither carried nor named acked: kept, not served as the primary's, counted.
    pub fn mirror_unvouched(&self) -> u64 {
        self.unvouched.lock().len() as u64
    }

    /// Full re-syncs this mirror ran because its digest differed from the primary's (ER6 PR B): a
    /// replicated `Post` or `Ack` that was lost.
    pub fn mirror_digest_resyncs(&self) -> u64 {
        self.digest_resyncs.load(Ordering::Relaxed)
    }

    /// A replicated `Ack` for `id`: the primary no longer holds it live (review I1).
    pub(crate) fn forget_refused_live(&self, id: u64) {
        self.refused_live.lock().remove(&id);
    }

    /// Replicated facts this node's store refused because their id is one it remembers acked
    /// (ER6) — a stale re-delivery, or another primary reusing an id. Counted, never re-synced.
    pub fn mirror_refused_acked(&self) -> u64 {
        self.store().map_or(0, |s| s.refused_acked())
    }

    /// Whether the mirror should re-sync from the primary now.
    /// Not while this mirror's WAL refuses appends: every fact would be refused again and the
    /// primary's whole board re-fetched per tick (#597 re-review, finding 2). The gap waits.
    fn should_resync(&self) -> bool {
        if self.store().is_some_and(|s| s.wal_refusing()) {
            return false;
        }
        self.mirror_gap.swap(false, Ordering::AcqRel)
    }

    /// The node a promotion should sync from first: the primary this mirror last heard from, when a
    /// refused fact is pending — it may still answer (a graceful step-down, a blip), and otherwise
    /// the refused fact is lost with it (#597 re-review, finding 3).
    fn promotion_sync_target(&self) -> Option<NodeId> {
        if !self.mirror_gap.load(Ordering::Acquire) {
            return None;
        }
        self.last_primary.lock().clone()
    }

    /// Remember the primary this mirror last heard from.
    pub(crate) fn note_primary_seen(&self, node: &NodeId) {
        *self.last_primary.lock() = Some(node.clone());
    }

    /// A mirrored fact's WAL append was refused (it was not applied, so a re-delivery applies it):
    /// ask the mirror to re-sync from the primary.
    pub(crate) fn note_mirror_gap(&self, id: u64, e: &BlackboardError) {
        tracing::error!(id, error = %e, "blackboard: mirror fact refused; it is re-fetched once the WAL is repaired");
        self.mirror_gap.store(true, Ordering::Release);
    }

    /// The store refused to open (a corrupt WAL, a second owner, an unreadable file): say why — the
    /// error names the file and, for corruption, the byte — and withdraw the candidacy, so the ring
    /// does not keep electing a node that cannot serve (#597 review, finding 2).
    fn withdraw_after_store_refusal(&self, e: &BlackboardError) {
        tracing::error!(
            ns = %self.cfg.namespace,
            wal = %self.cfg.wal_path.display(),
            error = %e,
            "blackboard: the store refused to open; withdrawing the candidacy and staying client"
        );
        *self.role_reg.lock() = None;
    }

    // ── Role assumption ──────────────────────────────────────────────────────

    fn become_primary(self: &Arc<Self>) {
        let store = self.store_expect();
        let ns = &self.cfg.namespace;
        let mut tasks = rpc::spawn_primary_handlers(self);

        let reg = self.agent.capabilities().advertise_capability(
            Capability::new("blackboard", format!("{ns}.primary")),
            self.cfg.cap_refresh,
        );
        *self.primary_reg.lock() = Some(reg);
        *self.role_reg.lock() = None; // retract candidate/secondary ad

        // Re-queue scan: claims not acked within the deadline return to claimable (at-least-once).
        {
            let store2 = Arc::clone(&store);
            let timeout = Duration::from_secs(self.cfg.claim_timeout_secs);
            tasks.push(tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(30));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    for id in store2.requeue_expired(timeout) {
                        tracing::warn!(id, "blackboard: re-queued expired in-flight claim");
                    }
                }
            }));
        }

        self.tasks.lock().extend(tasks);
        self.is_secondary.store(false, Ordering::Release);
        self.is_primary.store(true, Ordering::Release);
        tracing::info!(ns = %self.cfg.namespace, "blackboard: serving as primary");
    }

    fn become_secondary(self: &Arc<Self>) {
        let ns = &self.cfg.namespace;
        let mut tasks = rpc::spawn_mirror_handlers(self);

        let reg = self.agent.capabilities().advertise_capability(
            Capability::new("blackboard", format!("{ns}.secondary")),
            self.cfg.cap_refresh,
        );
        *self.role_reg.lock() = Some(reg);

        // Initial sync: pull the primary's current live facts so the mirror is COMPLETE before this
        // secondary could ever be promoted. Live Post/Ack replication only ships facts posted *while
        // this secondary is present*, so a single-shot sync that hit a transient-unresolvable primary
        // (fresh ring / just-elected primary) left a PARTIAL mirror — and a later failover silently
        // lost the pre-join backlog while the operator believed redundancy was restored. Retry each
        // interval until it drains once, or until this node promotes. Ported from the tuple-space
        // join-time backfill after audit 2026-07-15 pass 3 found the blackboard lacked the retry.
        {
            let me = Arc::clone(self);
            let interval = self.cfg.cap_refresh;
            tasks.push(tokio::spawn(async move {
                let mut tick = tokio::time::interval(interval);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    if me.is_primary() {
                        return; // promoted while waiting — it now owns the full board
                    }
                    if me.sync_from_primary().await {
                        tracing::info!(ns = %me.cfg.namespace, "blackboard: initial mirror sync drained");
                        // Drained once. Stay, to re-sync whenever a mirrored fact was refused
                        // (`mirror_gap`), so it is re-delivered after the WAL repairs — and, every
                        // `DIGEST_PROBE_TICKS`, to compare digests with the primary (ER6 PR B):
                        // `threshold` consecutive mismatches start a full sync, which repairs a lost
                        // `Post` or `Ack`. Replication in flight at probe time also mismatches, and
                        // no sync cures that: a sync that retired nothing and refetched nothing the
                        // primary issued before the streak began doubles `threshold`, up to
                        // `DIGEST_MISMATCHES_MAX`; one that repaired something resets it. Against a
                        // primary older than PR B, whose answer is a full snapshot, the probe backs
                        // off to one in `LEGACY_PROBE_BACKOFF`.
                        let mut ticks: u32 = 0;
                        let mut mismatches: u32 = 0;
                        let mut threshold = DIGEST_MISMATCHES_TO_SYNC;
                        let mut floor: Option<u64> = None;
                        let mut skip: u32 = 0;
                        loop {
                            tick.tick().await;
                            if me.is_primary() {
                                return;
                            }
                            if me.should_resync() {
                                break;
                            }
                            ticks = ticks.wrapping_add(1);
                            if !ticks.is_multiple_of(DIGEST_PROBE_TICKS) {
                                continue;
                            }
                            if skip > 0 {
                                skip -= 1;
                                continue;
                            }
                            match me.probe_primary().await {
                                Probe::Mismatch { next_id } => {
                                    mismatches += 1;
                                    // The primary's next id when the streak began: a fact below it
                                    // that a sync brings had time to arrive by replication.
                                    let streak_floor = *floor.get_or_insert(next_id);
                                    if mismatches >= threshold {
                                        me.digest_resyncs.fetch_add(1, Ordering::Relaxed);
                                        tracing::warn!(ns = %me.cfg.namespace, mismatches,
                                            "blackboard: the mirror's digest differs from the primary's — re-syncing");
                                        mismatches = 0;
                                        floor = None;
                                        match me.digest_resync(streak_floor).await {
                                            Some(0) => {
                                                threshold = (threshold * 2).min(DIGEST_MISMATCHES_MAX);
                                                tracing::info!(ns = %me.cfg.namespace, threshold,
                                                    "blackboard: the re-sync repaired nothing (replication in flight); backing the probe off");
                                            }
                                            Some(_) => threshold = DIGEST_MISMATCHES_TO_SYNC,
                                            None => {}
                                        }
                                    }
                                }
                                Probe::Match => {
                                    mismatches = 0;
                                    floor = None;
                                }
                                Probe::Legacy => {
                                    mismatches = 0;
                                    floor = None;
                                    skip = LEGACY_PROBE_BACKOFF - 1;
                                }
                                Probe::Unknown => {}
                            }
                        }
                    }
                    // Unreachable / partial: retry next tick.
                }
            }));
        }

        // Promotion watch: the capability ring IS the failure detector.
        //
        // "Evaporated" means *was there, then gone*. An empty resolve of `.primary` before this watch
        // has ever SEEN the primary's advertisement is startup propagation lag, not failure — on a
        // CPU-starved host the first sighting can take many intervals, and promoting on lag creates a
        // split-brain primary that never demotes (the tuple-space #150/S13 failure). Audit 2026-07-15
        // pass 3 found the blackboard promoted after just two empty resolves with neither a
        // seen-primary gate nor an orphan grace — porting the tuple-space guard. Before first sight
        // the watch promotes only after a much longer orphan grace: the primary may genuinely be dead
        // while this secondary (re)starts, so availability still needs a bounded path; propagation lag
        // does not survive 10 intervals.
        {
            let me = Arc::clone(self);
            let interval = self.cfg.cap_refresh;
            tasks.push(tokio::spawn(async move {
                let mut tick = tokio::time::interval(interval);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                let mut seen_primary = false;
                let mut unseen_ticks: u32 = 0;
                const ORPHAN_GRACE_TICKS: u32 = 10;
                loop {
                    tick.tick().await;
                    if !me.resolve_role("primary").is_empty() {
                        seen_primary = true;
                        continue;
                    }
                    if seen_primary {
                        // Was there, now gone — confirm one interval later (split-brain guard).
                        tokio::time::sleep(interval).await;
                        if !me.resolve_role("primary").is_empty() {
                            continue;
                        }
                        tracing::warn!(ns = %me.cfg.namespace, "blackboard: primary evaporated — promoting");
                    } else {
                        // Never seen: propagation lag until the orphan grace expires.
                        unseen_ticks += 1;
                        if unseen_ticks < ORPHAN_GRACE_TICKS {
                            continue;
                        }
                        tracing::warn!(ns = %me.cfg.namespace, ticks = ORPHAN_GRACE_TICKS,
                            "blackboard: no primary ever seen within the orphan grace — promoting");
                    }
                    if let Some(old) = me.promotion_sync_target() {
                        if me.sync_from(old).await {
                            me.mirror_gap.store(false, Ordering::Release);
                            tracing::info!(ns = %me.cfg.namespace, "blackboard: re-synced a pending gap before promoting");
                        } else {
                            tracing::error!(ns = %me.cfg.namespace,
                                "blackboard: promoting with a mirror gap the old primary could not fill — facts refused by this mirror's WAL may be missing");
                        }
                    }
                    me.become_primary();
                    return;
                }
            }));
        }

        self.tasks.lock().extend(tasks);
        self.is_secondary.store(true, Ordering::Release);
        tracing::info!(ns = %self.cfg.namespace, "blackboard: mirroring as secondary");
    }

    async fn run_election(self: Arc<Self>) {
        let ns = &self.cfg.namespace;
        // The candidate ad carries which election rules this build can compute, so the ring
        // negotiates rather than flag-days (`mycelium::election`). On the *candidate*, not the
        // primary: the negotiation is over who might win, not over who did.
        let (attr, value) = mycelium::election::rule_attribute();
        let reg = self.agent.capabilities().advertise_capability(
            Capability::new("blackboard", format!("{ns}.candidate")).with(attr, value),
            self.cfg.cap_refresh,
        );
        *self.role_reg.lock() = Some(reg);

        let settle = (self.cfg.cap_refresh * 2).max(Duration::from_secs(2));
        tokio::time::sleep(settle).await;

        loop {
            if !self.resolve_role("primary").is_empty() {
                match self.init_store() {
                    Ok(()) => self.become_secondary(),
                    Err(e) => self.withdraw_after_store_refusal(&e),
                }
                return;
            }
            // The ring's own name is what makes rendezvous spread: two rings must order the same
            // candidates differently, or every ring lands on one node (P10 — a coordinator nobody
            // declared).
            let ring = format!("blackboard/{ns}.primary");
            let candidates = self.resolve_role_pairs("candidate");
            let self_id = self.agent.node_id().to_string();
            match mycelium::election::elect(&ring, &candidates) {
                Some(winner) if winner.to_string() == self_id => {
                    match self.init_store() {
                        Ok(()) => self.become_primary(),
                        Err(e) => self.withdraw_after_store_refusal(&e),
                    }
                    return;
                }
                _ => tokio::time::sleep(self.cfg.cap_refresh).await,
            }
        }
    }

    /// Pull the primary's current live facts into the mirror (one snapshot RPC). Returns `true` iff a
    /// full snapshot was fetched and applied — the caller retries until this drains once, so a
    /// transient-unresolvable primary cannot leave a partial mirror (audit 2026-07-15 pass 3).
    async fn sync_from_primary(self: &Arc<Self>) -> bool {
        let Ok(primary) = self.resolve_primary() else { return false };
        self.sync_from(primary).await
    }

    /// One snapshot RPC to `primary`, applied into the mirror.
    async fn sync_from(self: &Arc<Self>, primary: NodeId) -> bool {
        match self.fetch_snapshot(primary).await {
            Some(resp) => self.apply_snapshot(&resp),
            None => false,
        }
    }

    /// A full snapshot from `primary`, or `None` if it did not answer.
    async fn fetch_snapshot(self: &Arc<Self>, primary: NodeId) -> Option<Bytes> {
        self.note_primary_seen(&primary);
        let kind = rpc::rpc_kind(&self.cfg.namespace, "snapshot");
        self.agent.service().rpc_call(primary, kind, Bytes::new(), Duration::from_secs(10)).await.ok()
    }

    /// The full sync a digest mismatch starts (review round 2): how many facts it repaired —
    /// retired, or refetched with an id below `floor` (issued before the mismatch streak began, so
    /// not merely in flight) — or `None` if no sync was applied.
    async fn digest_resync(self: &Arc<Self>, floor: u64) -> Option<u64> {
        let primary = self.resolve_primary().ok()?;
        let resp = self.fetch_snapshot(primary).await?;
        self.apply_snapshot_counted(&resp, floor)
    }

    /// One digest probe (ER6 PR B): ask the primary for its digest only, refresh the id fence from
    /// its next id, and compare its live set with this mirror's vouched facts. A primary older than
    /// PR B answers a full snapshot, which is applied as a sync.
    async fn probe_primary(self: &Arc<Self>) -> Probe {
        let Ok(primary) = self.resolve_primary() else { return Probe::Unknown };
        let kind = rpc::rpc_kind(&self.cfg.namespace, "snapshot");
        let Ok(resp) = self
            .agent
            .service()
            .rpc_call(primary, kind, Bytes::from_static(&[rpc::SNAP_REQ_DIGEST]), Duration::from_secs(10))
            .await
        else {
            return Probe::Unknown;
        };
        let Ok(decoded) = rpc::dec_snapshot_resp(&resp) else { return Probe::Unknown };
        let Some(digest) = decoded.digest else {
            self.apply_snapshot(&resp);
            return Probe::Legacy;
        };
        self.compare_digest(decoded.trailer.map(|t| t.next_id), digest)
    }

    /// Fence past the primary's next id, then compare its digest with this mirror's over vouched
    /// facts. The unvouched set is cloned and released before the store's lock (row 29 then row 27,
    /// never nested).
    pub(crate) fn compare_digest(&self, next_id: Option<u64>, digest: rpc::SnapshotDigest) -> Probe {
        let Some(store) = self.store() else { return Probe::Unknown };
        if let Some(n) = next_id
            && let Err(e) = store.fence_past(n)
        {
            tracing::error!(next_id = n, error = %e, "blackboard: the mirror's id fence was set but not logged");
        }
        let unvouched = self.unvouched.lock().clone();
        let refused = self.refused_live.lock().clone();
        let (mut count, mut mix) = store.live_digest(&unvouched);
        // Held live on the primary, refused here as acked (review I1): counted as the primary's,
        // since no sync will bring them.
        for id in refused {
            count += 1;
            mix ^= store::digest_mix(id);
        }
        if (count, mix) == (digest.count, digest.mix) { Probe::Match } else { Probe::Mismatch { next_id: next_id.unwrap_or(0) } }
    }

    /// Apply a primary's snapshot response into this mirror (ER6, D6a). **Positive evidence, never
    /// absence:** a fact the snapshot lacks is kept unless the primary names its id acked — a mirror
    /// that holds a fact the primary never did (a promoted mirror's, an unvouched one) keeps it. In
    /// order: each id the trailer names acked is discarded (and remembered, so the facts below cannot
    /// bring it back); the facts are added; the id fence moves past the primary's next id. A snapshot
    /// with no trailer (a primary older than ER6) only adds, as before. `true` iff it applied.
    pub(crate) fn apply_snapshot(&self, resp: &Bytes) -> bool {
        self.apply_snapshot_counted(resp, 0).is_some()
    }

    /// [`apply_snapshot`](Self::apply_snapshot), returning how many facts it repaired: retired,
    /// or applied with an id below `floor`. `None` if nothing was applied.
    fn apply_snapshot_counted(&self, resp: &Bytes, floor: u64) -> Option<u64> {
        let (Ok(rpc::DecodedSnapshot { facts, trailer, .. }), Some(store)) = (rpc::dec_snapshot_resp(resp), self.store()) else {
            return None;
        };
        let mut repaired = 0u64;
        let carried: HashSet<u64> = facts.iter().map(|f| f.id).collect();
        if let Some(t) = &trailer {
            let mut retired = 0u64;
            for id in t.acked_ids() {
                match store.discard(id) {
                    Ok(true) => retired += 1,
                    Ok(false) => {}
                    // The fact stays live in the log (at-least-once); the memory holds the id, so
                    // it is not served again before a restart.
                    Err(e) => tracing::error!(id, error = %e, "blackboard: a retired mirror fact's ack not logged"),
                }
            }
            repaired += retired;
            if retired > 0 {
                self.mirror_retired.fetch_add(retired, Ordering::Relaxed);
                tracing::info!(ns = %self.cfg.namespace, retired, "blackboard: retired mirrored facts the primary reports acked");
            }
        }
        let mut refused = HashSet::new();
        for f in facts {
            let id = f.id;
            match store.apply_with_id(id, f.attributes, f.payload) {
                Ok(store::PostOutcome::Refused) => {
                    refused.insert(id); // counted by the store
                }
                Ok(store::PostOutcome::Applied) if id < floor => repaired += 1,
                Ok(_) => {}
                Err(e) => self.note_mirror_gap(id, &e),
            }
        }
        *self.refused_live.lock() = refused;
        if let Some(t) = &trailer {
            if let Err(e) = store.fence_past(t.next_id) {
                tracing::error!(next_id = t.next_id, error = %e, "blackboard: the mirror's id fence was set but not logged");
            }
            // What this mirror holds that the primary neither carried nor named acked (D6c): kept and
            // counted, and left out of the digest. Only an ER6 primary's snapshot is complete enough
            // to say so; facts replicated after it was read are vouched by their own `Post`.
            let unvouched: HashSet<u64> = store.live_ids().into_iter().filter(|id| !carried.contains(id) && *id < t.next_id).collect();
            if !unvouched.is_empty() {
                tracing::warn!(ns = %self.cfg.namespace, unvouched = unvouched.len(),
                    "blackboard: the mirror holds facts the primary cannot vouch for; kept (at-least-once)");
            }
            *self.unvouched.lock() = unvouched;
        }
        Some(repaired) // a full snapshot (possibly empty) was applied — the mirror is now complete
    }

    // ── Serving paths (primary) ──────────────────────────────────────────────

    pub(crate) fn serve_post(self: &Arc<Self>, attrs: BTreeMap<String, String>, payload: Bytes) -> Result<u64, BlackboardError> {
        let store = self.store_expect();
        let id = store.post(attrs.clone(), payload.clone())?;
        self.replicate(rpc::enc_replicate_post(&Fact { id, attributes: attrs, payload }));
        Ok(id)
    }
    pub(crate) fn serve_read(&self, pred: &Predicate) -> Vec<Fact> {
        self.store().map(|s| s.read(pred)).unwrap_or_default()
    }
    pub(crate) fn serve_claim(&self, pred: &Predicate) -> Result<Option<Fact>, BlackboardError> {
        self.store_expect().claim(pred)
    }
    pub(crate) fn serve_ack(self: &Arc<Self>, id: u64) -> Result<(), BlackboardError> {
        self.store_expect().ack(id)?;
        self.replicate(rpc::enc_replicate_ack(id));
        Ok(())
    }
    pub(crate) fn serve_release(&self, id: u64) -> Result<(), BlackboardError> {
        self.store_expect().release(id)
    }

    /// Fire-and-forget replication of a `Post`/`Ack` record to every live secondary.
    fn replicate(self: &Arc<Self>, body: Bytes) {
        let secondaries = self.resolve_role("secondary");
        if secondaries.is_empty() {
            return;
        }
        let kind = rpc::rpc_kind(&self.cfg.namespace, "replicate");
        let agent = Arc::clone(&self.agent);
        #[cfg(test)]
        let delay = self.replicate_delay_ms.load(Ordering::Relaxed);
        tokio::spawn(async move {
            #[cfg(test)]
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            for node in secondaries {
                for _ in 0..2 {
                    if agent
                        .service()
                        .rpc_call(node.clone(), Arc::clone(&kind), body.clone(), Duration::from_secs(5))
                        .await
                        .is_ok()
                    {
                        break;
                    }
                }
            }
        });
    }

    // ── Resolution ───────────────────────────────────────────────────────────

    fn resolve_role(&self, role: &str) -> Vec<NodeId> {
        self.resolve_role_pairs(role).into_iter().map(|(n, _)| n).collect()
    }
    /// The same resolve, keeping each candidate's `Capability` — the election needs the advertised
    /// rule attribute, which `resolve_role` throws away.
    fn resolve_role_pairs(&self, role: &str) -> Vec<(NodeId, Capability)> {
        let filter = CapFilter::new("blackboard", format!("{}.{role}", self.cfg.namespace));
        self.agent.capabilities().resolve(&filter)
    }
    fn resolve_primary(&self) -> Result<NodeId, BlackboardError> {
        let mut providers = self.resolve_role("primary");
        if providers.is_empty() {
            return Err(BlackboardError::NoProvider);
        }
        providers.sort_by_key(NodeId::to_string);
        Ok(providers.remove(0))
    }
    fn serving_locally(&self) -> bool {
        self.is_primary.load(Ordering::Acquire)
    }

    /// `true` if this node is currently serving as the namespace **primary**.
    pub fn is_primary(&self) -> bool {
        self.is_primary.load(Ordering::Acquire)
    }

    /// `true` if this node is currently mirroring as a **secondary**.
    pub fn is_secondary(&self) -> bool {
        self.is_secondary.load(Ordering::Acquire)
    }

    // ── Public API ───────────────────────────────────────────────────────────

    /// Post a fact (Linda `out`) — non-destructive; readable and claimable cluster-wide. Returns its id.
    pub async fn post(self: &Arc<Self>, attributes: BTreeMap<String, String>, payload: Bytes) -> Result<u64, BlackboardError> {
        if self.serving_locally() {
            return self.serve_post(attributes, payload);
        }
        let primary = self.resolve_primary()?;
        let resp = self
            .agent
            .service()
            .rpc_call(primary, rpc::rpc_kind(&self.cfg.namespace, "post"), rpc::enc_post_req(&attributes, &payload), Duration::from_secs(10))
            .await
            .map_err(|e| BlackboardError::Rpc(e.to_string()))?;
        rpc::dec_id_resp(&resp)
    }

    /// Non-destructive read (Linda `rd`): all facts matching `predicate`.
    pub async fn read(self: &Arc<Self>, predicate: &Predicate) -> Result<Vec<Fact>, BlackboardError> {
        if self.serving_locally() {
            return Ok(self.serve_read(predicate));
        }
        let primary = self.resolve_primary()?;
        let resp = self
            .agent
            .service()
            .rpc_call(primary, rpc::rpc_kind(&self.cfg.namespace, "read"), rpc::enc_predicate_req(predicate), Duration::from_secs(10))
            .await
            .map_err(|e| BlackboardError::Rpc(e.to_string()))?;
        rpc::dec_facts_resp(&resp)
    }

    /// Competitive destructive claim (Linda `in`): claim one fact matching `predicate`, or `None`.
    pub async fn claim(self: &Arc<Self>, predicate: &Predicate) -> Result<Option<Fact>, BlackboardError> {
        if self.serving_locally() {
            return self.serve_claim(predicate);
        }
        let primary = self.resolve_primary()?;
        let resp = self
            .agent
            .service()
            .rpc_call(primary, rpc::rpc_kind(&self.cfg.namespace, "claim"), rpc::enc_predicate_req(predicate), Duration::from_secs(10))
            .await
            .map_err(|e| BlackboardError::Rpc(e.to_string()))?;
        rpc::dec_opt_fact_resp(&resp)
    }

    /// [`claim`](Self::claim) through the claim door (adopter-surface Y11): decided under `bb.claim` on `bb:{ns}`, this
    /// node the actor, when an exposure rule selects it — `causal` on the door's own decision record, and nowhere
    /// further. Untouched otherwise. A refusal is [`BlackboardError::Refused`], and nothing was claimed.
    pub async fn claim_in(
        self: &Arc<Self>,
        predicate: &Predicate,
        causal: &mycelium::causal::CausalFields,
    ) -> Result<Option<Fact>, BlackboardError> {
        let pass = self
            .agent
            .exposure_door_as_node(mycelium::door::OP_BB_CLAIM, &claim_resource(&self.cfg.namespace), causal, "bb/claim")
            .await
            .map_err(BlackboardError::Refused)?;
        let claimed = self.claim(predicate).await;
        pass.finish(claim_outcome(&claimed)).await;
        claimed
    }

    /// Terminal ack: the claimed fact was consumed.
    pub async fn ack(self: &Arc<Self>, id: u64) -> Result<(), BlackboardError> {
        if self.serving_locally() {
            return self.serve_ack(id);
        }
        let primary = self.resolve_primary()?;
        let resp = self
            .agent
            .service()
            .rpc_call(primary, rpc::rpc_kind(&self.cfg.namespace, "ack"), rpc::enc_id_req(id), Duration::from_secs(10))
            .await
            .map_err(|e| BlackboardError::Rpc(e.to_string()))?;
        rpc::dec_unit_resp(&resp)
    }

    /// Release: abandon a claim — the fact returns to claimable.
    pub async fn release(self: &Arc<Self>, id: u64) -> Result<(), BlackboardError> {
        if self.serving_locally() {
            return self.serve_release(id);
        }
        let primary = self.resolve_primary()?;
        let resp = self
            .agent
            .service()
            .rpc_call(primary, rpc::rpc_kind(&self.cfg.namespace, "release"), rpc::enc_id_req(id), Duration::from_secs(10))
            .await
            .map_err(|e| BlackboardError::Rpc(e.to_string()))?;
        rpc::dec_unit_resp(&resp)
    }

    /// Live depth of the board (claimable + in-flight).
    pub async fn depth(self: &Arc<Self>) -> Result<BoardDepth, BlackboardError> {
        if self.serving_locally() {
            return Ok(self.store_expect().depth());
        }
        let primary = self.resolve_primary()?;
        let resp = self
            .agent
            .service()
            .rpc_call(primary, rpc::rpc_kind(&self.cfg.namespace, "depth"), Bytes::new(), Duration::from_secs(10))
            .await
            .map_err(|e| BlackboardError::Rpc(e.to_string()))?;
        rpc::dec_depth_resp(&resp)
    }

    /// Abort background tasks and retract advertisements.
    pub async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        for h in self.tasks.lock().drain(..) {
            h.abort();
        }
        *self.primary_reg.lock() = None;
        *self.role_reg.lock() = None;
        self.is_primary.store(false, Ordering::Release);
        self.is_secondary.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod wal_role_tests {
    use super::*;
    use crate::store::BoardStore;
    use mycelium::GossipConfig;

    async fn agent() -> Arc<GossipAgent> {
        let port = mycelium::test_util::alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        agent.start().await.unwrap();
        agent
    }

    fn temp_wal(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mbb-role-{}-{name}.log", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn fact(id: u64) -> Fact {
        Fact {
            id,
            attributes: BTreeMap::from([("kind".to_string(), "surplus".to_string())]),
            payload: Bytes::from(vec![id as u8; 64]),
        }
    }

    /// #597 review, finding 1 (a regression the PR introduced): a secondary's WAL poisoned by one
    /// failed append was never repaired — only the primary ran compaction — and the replicate handler
    /// had already marked each refused id mirrored, so no re-delivery applied it: failover lost every
    /// fact after the error. The mirror must repair, and a re-delivery must land, in memory and in the
    /// log.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_poisoned_mirror_repairs_and_keeps_every_later_record() {
        let path = temp_wal("poisoned-mirror");
        let bb = Blackboard::new(
            agent().await,
            BoardConfig {
                namespace: Arc::from("poisoned-mirror"),
                role: BoardRole::Secondary,
                persist: true,
                wal_path: path.clone(),
                cap_refresh: Duration::from_millis(300),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let store = bb.store().unwrap();
        store.inject_append_fault();
        for id in 0..4 {
            bb.apply_replicated(&rpc::enc_replicate_post(&fact(id)));
        }
        for _ in 0..50 {
            if !store.wal_poisoned() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // The re-delivery (a re-sent replicate, or the re-sync a gap triggers).
        for id in 0..4 {
            bb.apply_replicated(&rpc::enc_replicate_post(&fact(id)));
        }
        let mut ids: Vec<u64> = store.read(&Predicate::new()).iter().map(|f| f.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![0, 1, 2, 3], "every fact is in the mirror");
        bb.shutdown().await;
        drop(store);
        drop(bb);
        let _ = std::fs::remove_file(&path);
    }

    /// #597 review, finding 2: in Auto mode an elected node whose WAL refuses to open logged nothing
    /// and kept advertising its candidacy, so the ring elected it again and never got a primary.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_auto_node_whose_wal_refuses_withdraws_its_candidacy() {
        let path = temp_wal("auto-refused");
        let mut bytes = b"MBBWAL".to_vec();
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.push(99);
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        std::fs::write(&path, &bytes).unwrap();
        let bb = Blackboard::new(
            agent().await,
            BoardConfig {
                namespace: Arc::from("auto-refused"),
                role: BoardRole::Auto,
                persist: true,
                wal_path: path.clone(),
                cap_refresh: Duration::from_millis(300),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        for _ in 0..50 {
            if bb.role_reg.lock().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(bb.role_reg.lock().is_some(), "the node advertises its candidacy first");
        let mut withdrawn = false;
        for _ in 0..60 {
            if bb.role_reg.lock().is_none() {
                withdrawn = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(withdrawn, "the candidacy is withdrawn when the store refuses");
        assert!(!bb.is_primary() && !bb.is_secondary());
        bb.shutdown().await;
        let _ = std::fs::remove_file(&path);
    }

    fn mirror_cfg(ns: &str, persist: bool, path: PathBuf) -> BoardConfig {
        BoardConfig {
            namespace: Arc::from(ns),
            role: BoardRole::Secondary,
            persist,
            wal_path: path,
            cap_refresh: Duration::from_millis(300),
            ..Default::default()
        }
    }

    /// #597 re-review, finding 1 (the board's form): an `Ack` that arrives before the snapshot carrying
    /// its fact (a re-sync racing live replication) found nothing to discard, and the snapshot's `Post`
    /// then resurrected the acked fact in the mirror.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_ack_before_its_post_does_not_resurrect_the_fact() {
        let bb = Blackboard::new(agent().await, mirror_cfg("ack-first", false, temp_wal("unused"))).await.unwrap();
        bb.apply_replicated(&rpc::enc_replicate_post(&fact(1)));
        bb.apply_replicated(&rpc::enc_replicate_ack(0));
        bb.apply_replicated(&rpc::enc_replicate_post(&fact(0)));
        let ids: Vec<u64> = bb.store().unwrap().read(&Predicate::new()).iter().map(|f| f.id).collect();
        assert_eq!(ids, vec![1], "the acked fact stays acked in the mirror");
        bb.shutdown().await;
    }

    /// #597 re-review, finding 2: no re-sync while the mirror's WAL refuses appends.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_resync_while_the_mirror_wal_refuses() {
        let path = temp_wal("no-resync-poisoned");
        let bb = Blackboard::new(agent().await, mirror_cfg("no-resync", true, path.clone())).await.unwrap();
        let store = bb.store().unwrap();
        store.set_compact_fault(true);
        store.inject_append_fault();
        assert!(store.post_with_id(0, BTreeMap::new(), Bytes::from("x")).is_err());
        bb.mirror_gap.store(true, Ordering::Release);
        assert!(!bb.should_resync(), "no re-sync while the WAL refuses appends");
        assert!(bb.mirror_gap.load(Ordering::Acquire), "the gap waits for the repair");
        store.set_compact_fault(false);
        bb.shutdown().await;
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    /// #597 re-review, finding 3 (the board's form): a promotion with a gap pending first syncs from the
    /// primary this mirror last heard from, which may still answer (a graceful step-down, a blip).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_promotion_with_a_pending_gap_syncs_from_the_last_primary() {
        let bb = Blackboard::new(agent().await, mirror_cfg("gap-promotion", false, temp_wal("unused2"))).await.unwrap();
        let old = NodeId::new("127.0.0.1", 1).unwrap();
        bb.note_primary_seen(&old);
        assert_eq!(bb.promotion_sync_target(), None, "no gap, no extra sync");
        bb.mirror_gap.store(true, Ordering::Release);
        assert_eq!(bb.promotion_sync_target(), Some(old), "a gap syncs from the last primary first");
        bb.shutdown().await;
    }

    /// #597 re-review, finding 6: `init_store` racing `shutdown` started a maintenance task nothing
    /// aborted, which held the store and so the WAL's ownership lock.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_store_opened_after_shutdown_does_not_pin_the_wal() {
        let path = temp_wal("init-after-shutdown");
        let mut cfg = mirror_cfg("late-init", true, path.clone());
        cfg.role = BoardRole::Client;
        let bb = Blackboard::new(agent().await, cfg).await.unwrap();
        bb.shutdown().await;
        bb.init_store().unwrap();
        drop(bb);
        let mut reopened = false;
        for _ in 0..30 {
            if BoardStore::persistent(&path, 1).is_ok() {
                reopened = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(reopened, "the WAL's lock is released once the board is gone");
        let _ = std::fs::remove_file(&path);
    }

    /// #597 re-review, finding 7: the mirror's sync loop re-syncs on `mirror_gap` — a primary and a
    /// mirror, one injected append failure on the mirror, the refused fact arriving by the re-sync.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_mirror_resyncs_a_refused_fact() {
        let a = agent().await;
        let port = mycelium::test_util::alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = vec![a.node_id().clone()];
        let b = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        b.start().await.unwrap();
        let path = temp_wal("resync-loop");
        let mut pcfg = mirror_cfg("resync-loop", false, temp_wal("unused3"));
        pcfg.role = BoardRole::Primary;
        let primary = Blackboard::new(a, pcfg).await.unwrap();
        let mirror = Blackboard::new(b, mirror_cfg("resync-loop", true, path.clone())).await.unwrap();
        let mstore = mirror.store().unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        mstore.inject_append_fault();
        let id = primary.post(fact(7).attributes, Bytes::from("refused-then-resynced")).await.unwrap();
        let mut arrived = false;
        for _ in 0..100 {
            if mstore.read(&Predicate::new()).iter().any(|f| f.id == id) {
                arrived = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(arrived, "fact {id} reached the mirror by the re-sync");
        mirror.shutdown().await;
        primary.shutdown().await;
        drop(mstore);
        let _ = std::fs::remove_file(&path);
    }

    /// ER3 (R4-01), the board's mirror: the mirror refuses an id it saw acked (the index's `admit`
    /// before ER6, the store's acked-id memory since). A transient primary restarted in the same namespace reissued ids from 0, so the mirror
    /// silently refused its new facts. With a fresh id base they land.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_restarted_transient_primary_is_not_refused_by_a_mirror_that_saw_its_old_ids_acked() {
        let bb = Blackboard::new(agent().await, mirror_cfg("restarted-primary", false, temp_wal("unused-restart")))
            .await
            .unwrap();
        let with_id = |id: u64| Fact { id, ..fact(0) };
        // The primary's first life: one fact acked, one still live when it stops.
        let old = BoardStore::transient();
        let acked = old.post(fact(0).attributes, Bytes::from("old acked")).unwrap();
        let live = old.post(fact(0).attributes, Bytes::from("old live")).unwrap();
        bb.apply_replicated(&rpc::enc_replicate_post(&with_id(acked)));
        bb.apply_replicated(&rpc::enc_replicate_post(&with_id(live)));
        bb.apply_replicated(&rpc::enc_replicate_ack(acked));
        drop(old);
        // Its second life, from a fresh transient store.
        let new = BoardStore::transient();
        let a = new.post(fact(0).attributes, Bytes::from("new a")).unwrap();
        let b = new.post(fact(0).attributes, Bytes::from("new b")).unwrap();
        bb.apply_replicated(&rpc::enc_replicate_post(&with_id(a)));
        bb.apply_replicated(&rpc::enc_replicate_post(&with_id(b)));
        let mut ids: Vec<u64> = bb.store().unwrap().read(&Predicate::new()).iter().map(|f| f.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![live, a, b], "the old live fact and both new facts are mirrored");
        bb.shutdown().await;
    }


    fn snapshot_of(store: &BoardStore) -> Bytes {
        Bytes::from(rpc::snapshot_to_bytes(&store.mirror_snapshot()))
    }

    /// ER6 (D6a): a fact the primary names acked is retired from the mirror, counted, and the id
    /// fence moves past the primary's next id.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_snapshot_retires_what_the_primary_names_acked() {
        let bb = Blackboard::new(agent().await, mirror_cfg("retire", false, temp_wal("unused-retire"))).await.unwrap();
        let primary = BoardStore::transient();
        let done = primary.post(fact(0).attributes, Bytes::from("done")).unwrap();
        bb.apply_replicated(&rpc::enc_replicate_post(&Fact { id: done, ..fact(0) }));
        primary.claim(&Predicate::new()).unwrap();
        primary.ack(done).unwrap(); // the replicated Ack is lost
        let later = primary.post(fact(0).attributes, Bytes::from("later")).unwrap();
        primary.claim(&Predicate::new()).unwrap();
        primary.ack(later).unwrap(); // acked before the mirror ever heard of it
        assert!(bb.apply_snapshot(&snapshot_of(&primary)));
        let store = bb.store().unwrap();
        assert!(store.read(&Predicate::new()).is_empty(), "the acked fact is retired");
        assert_eq!(bb.mirror_retired(), 1);
        let next = store.post(fact(0).attributes, Bytes::new()).unwrap();
        assert!(next > later, "the mirror issued {next}, at or below {later}");
        bb.shutdown().await;
    }

    /// ER6, the control for the rule: a snapshot taken before a post, applied after the post was
    /// replicated, lacks the fact — and does not discard it (absence is not an ack).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_snapshot_racing_a_post_never_discards_it() {
        let bb = Blackboard::new(agent().await, mirror_cfg("race", false, temp_wal("unused-race"))).await.unwrap();
        let primary = BoardStore::transient();
        primary.post(fact(0).attributes, Bytes::from("a")).unwrap();
        let stale = snapshot_of(&primary);
        let b = primary.post(fact(0).attributes, Bytes::from("b")).unwrap();
        bb.apply_replicated(&rpc::enc_replicate_post(&Fact { id: b, ..fact(0) }));
        assert!(bb.apply_snapshot(&stale));
        let ids: Vec<u64> = bb.store().unwrap().read(&Predicate::new()).iter().map(|f| f.id).collect();
        assert!(ids.contains(&b), "the post the snapshot did not see survives: {ids:?}");
        assert_eq!(ids.len(), 2);
        bb.shutdown().await;
    }

    /// ER6 (D6c), the guard against the absence rule: a mirror holds a fact this primary never held
    /// (a fact from an earlier primary that this one, restarted empty, never saw acked). The snapshot
    /// lacks it and names it nowhere: it is kept.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unacked_fact_the_primary_never_held_is_not_discarded() {
        let bb = Blackboard::new(agent().await, mirror_cfg("never-held", false, temp_wal("unused-never"))).await.unwrap();
        let earlier = BoardStore::transient();
        let only_copy = earlier.post(fact(0).attributes, Bytes::from("only copy")).unwrap();
        bb.apply_replicated(&rpc::enc_replicate_post(&Fact { id: only_copy, ..fact(0) }));
        drop(earlier);
        let restarted = BoardStore::transient();
        let x = restarted.post(fact(0).attributes, Bytes::from("x")).unwrap();
        restarted.claim(&Predicate::new()).unwrap();
        restarted.ack(x).unwrap();
        assert!(bb.apply_snapshot(&snapshot_of(&restarted)));
        let ids: Vec<u64> = bb.store().unwrap().read(&Predicate::new()).iter().map(|f| f.id).collect();
        assert_eq!(ids, vec![only_copy], "the mirror's only copy is kept");
        assert_eq!(bb.mirror_retired(), 0);
        bb.shutdown().await;
    }

    /// A primary on one agent and a persistent-or-not mirror on a second, peered; the mirror's
    /// initial sync drained.
    async fn primary_and_mirror(ns: &str) -> (Arc<Blackboard>, Arc<Blackboard>) {
        let a = agent().await;
        let port = mycelium::test_util::alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = vec![a.node_id().clone()];
        let b = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        b.start().await.unwrap();
        let mut pcfg = mirror_cfg(ns, false, temp_wal("unused-pb"));
        pcfg.role = BoardRole::Primary;
        let primary = Blackboard::new(a, pcfg).await.unwrap();
        let mirror = Blackboard::new(b, mirror_cfg(ns, false, temp_wal("unused-mb"))).await.unwrap();
        let id = primary.post(fact(0).attributes, Bytes::from("handshake")).await.unwrap();
        let mstore = mirror.store().unwrap();
        for _ in 0..200 {
            if mstore.read(&Predicate::new()).iter().any(|f| f.id == id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(mstore.read(&Predicate::new()).iter().any(|f| f.id == id), "the mirror is replicating");
        (primary, mirror)
    }

    async fn eventually(what: &str, cond: impl Fn() -> bool) {
        for _ in 0..300 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{what}: not within 15 s");
    }

    /// ER6 PR B: a replicated `Ack` that never arrives (here: the primary acks in its store, bypassing
    /// replication) left the fact live on the mirror until a restart. The digest probe finds the
    /// difference and a full sync retires it, with the mirror still running.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_lost_replicated_ack_is_reconciled_without_a_restart() {
        let (primary, mirror) = primary_and_mirror("lost-ack").await;
        let id = primary.post(fact(0).attributes, Bytes::from("x")).await.unwrap();
        let mstore = mirror.store().unwrap();
        eventually("the post replicates", || mstore.read(&Predicate::new()).iter().any(|f| f.id == id)).await;
        let pstore = primary.store().unwrap();
        let claimed = pstore.claim(&Predicate::new()).unwrap().unwrap();
        pstore.ack(claimed.id).unwrap();
        eventually("the mirror retires the fact the primary acked", || {
            !mstore.read(&Predicate::new()).iter().any(|f| f.id == claimed.id)
        })
        .await;
        assert!(mirror.mirror_digest_resyncs() >= 1);
        mirror.shutdown().await;
        primary.shutdown().await;
    }

    /// ER6 PR B: a replicated `Post` that never arrives is refetched by the probe's full sync.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_lost_replicated_post_is_refetched() {
        let (primary, mirror) = primary_and_mirror("lost-post").await;
        let id = primary.store().unwrap().post(fact(0).attributes, Bytes::from("unreplicated")).unwrap();
        let mstore = mirror.store().unwrap();
        eventually("the mirror refetches the post", || mstore.read(&Predicate::new()).iter().any(|f| f.id == id)).await;
        mirror.shutdown().await;
        primary.shutdown().await;
    }

    /// ER6 (the third node): a mirror that saw an id acked refuses a later replicated `Post` under
    /// that id — before ER6 silently (the index's `admit`); now counted. With the fence a promoted
    /// mirror does not reissue the id, so the count stays 0 in a healthy fleet.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_third_node_does_not_silently_refuse_a_new_fact() {
        let bb = Blackboard::new(agent().await, mirror_cfg("third-node", false, temp_wal("unused-third"))).await.unwrap();
        let first = BoardStore::transient();
        let x = first.post(fact(0).attributes, Bytes::from("x")).unwrap();
        bb.apply_replicated(&rpc::enc_replicate_post(&Fact { id: x, ..fact(0) }));
        bb.apply_replicated(&rpc::enc_replicate_ack(x));
        // A promoted mirror that fenced from a snapshot does not reissue x ...
        let promoted = BoardStore::transient();
        let snap = first.mirror_snapshot();
        let resp = Bytes::from(rpc::snapshot_to_bytes(&snap));
        let rpc::DecodedSnapshot { trailer, .. } = rpc::dec_snapshot_resp(&resp).unwrap();
        promoted.fence_past(trailer.unwrap().next_id).unwrap();
        let fresh = promoted.post(fact(0).attributes, Bytes::from("new")).unwrap();
        assert!(fresh > x);
        bb.apply_replicated(&rpc::enc_replicate_post(&Fact { id: fresh, ..fact(0) }));
        assert_eq!(bb.mirror_refused_acked(), 0);
        // ... and if one is forced, it is counted, not dropped in silence.
        bb.apply_replicated(&rpc::enc_replicate_post(&Fact { id: x, ..fact(0) }));
        assert_eq!(bb.mirror_refused_acked(), 1);
        let ids: Vec<u64> = bb.store().unwrap().read(&Predicate::new()).iter().map(|f| f.id).collect();
        assert_eq!(ids, vec![fresh]);
        bb.shutdown().await;
    }

    /// ER6 PR B (D6c, correction 5): a fact the primary cannot vouch for is kept, counted, and left
    /// out of the digest — so it does not make every probe a mismatch.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unvouched_fact_is_kept_counted_and_out_of_the_digest() {
        let bb = Blackboard::new(agent().await, mirror_cfg("unvouched", false, temp_wal("unused-unv"))).await.unwrap();
        let earlier = BoardStore::transient();
        let orphan = earlier.post(fact(0).attributes, Bytes::from("orphan")).unwrap();
        bb.apply_replicated(&rpc::enc_replicate_post(&Fact { id: orphan, ..fact(0) }));
        let primary = BoardStore::transient();
        let live = primary.post(fact(0).attributes, Bytes::from("live")).unwrap();
        assert!(bb.apply_snapshot(&snapshot_of(&primary)));
        assert_eq!(bb.mirror_unvouched(), 1);
        let ids: Vec<u64> = bb.store().unwrap().read(&Predicate::new()).iter().map(|f| f.id).collect();
        assert_eq!(ids, vec![orphan, live]);
        let (count, mix) = primary.live_digest(&HashSet::new());
        let digest = rpc::SnapshotDigest { count, mix };
        assert_eq!(bb.compare_digest(None, digest), Probe::Match, "the unvouched fact is not in the digest");
        bb.shutdown().await;
    }

    /// Review I1: a node restarted with a tombstone for X becomes the secondary of a primary where
    /// X is live (X's replicated `Ack` was lost before the old primary died). The mirror refuses X
    /// at every sync, so its digest lacked X for ever and an idle board re-synced every two
    /// probes, the refusal count climbing. The refused-but-live ids are folded into the digest.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_refused_live_id_does_not_resync_for_ever() {
        let a = agent().await;
        let port = mycelium::test_util::alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = vec![a.node_id().clone()];
        let b = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        b.start().await.unwrap();
        let mut pcfg = mirror_cfg("refused-live", false, temp_wal("unused-rl"));
        pcfg.role = BoardRole::Primary;
        let primary = Blackboard::new(a, pcfg).await.unwrap();
        let x = primary.store().unwrap().post(fact(0).attributes, Bytes::from("x")).unwrap();
        let mwal = temp_wal("refused-live-mirror");
        {
            let restarted = BoardStore::persistent(&mwal, 1).unwrap();
            restarted.discard(x).unwrap();
        }
        let mirror = Blackboard::new(b, mirror_cfg("refused-live", true, mwal.clone())).await.unwrap();
        eventually("the initial sync refuses x", || mirror.mirror_refused_acked() >= 1).await;
        // Well past the probe count that starts a sync (each probe is 3 × 300 ms).
        tokio::time::sleep(Duration::from_secs(8)).await;
        assert_eq!(mirror.mirror_digest_resyncs(), 0, "a refusal no sync can cure started syncs");
        assert_eq!(mirror.mirror_refused_acked(), 1, "and the refusals climbed");
        mirror.shutdown().await;
        primary.shutdown().await;
        drop(mirror);
        let _ = std::fs::remove_file(&mwal);
    }

    /// Review I3: the streak was reset whenever the replication sequence moved, so a board with a
    /// write between every two probes never repaired a lost `Ack`. Now K consecutive mismatches
    /// start a sync, whatever the sequence.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_lost_ack_under_steady_writes_is_repaired_within_the_bound() {
        let (primary, mirror) = primary_and_mirror("steady-writes").await;
        let mstore = mirror.store().unwrap();
        let pstore = primary.store().unwrap();
        let claimed = pstore.claim(&Predicate::new()).unwrap().unwrap();
        pstore.ack(claimed.id).unwrap(); // its replicated Ack is lost
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (primary, stop) = (Arc::clone(&primary), Arc::clone(&stop));
            tokio::spawn(async move {
                while !stop.load(Ordering::Relaxed) {
                    let _ = primary.post(BTreeMap::from([("kind".to_string(), "steady".to_string())]), Bytes::new()).await;
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
            })
        };
        eventually("the lost ack is repaired under steady writes", || {
            !mstore.read(&Predicate::new()).iter().any(|f| f.id == claimed.id)
        })
        .await;
        stop.store(true, Ordering::Relaxed);
        let _ = writer.await;
        mirror.shutdown().await;
        primary.shutdown().await;
    }

    /// Review round 2: replication always in flight at probe time (here: held 2 s), nothing lost.
    /// Every probe mismatches and no sync can cure it; with a fixed threshold that was a full
    /// snapshot every four probes for as long as the load lasted. A sync that repairs nothing
    /// doubles the threshold.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn steady_writes_with_no_loss_back_the_probe_off() {
        let (primary, mirror) = primary_and_mirror("probe-backoff").await;
        primary.replicate_delay_ms.store(2_000, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (primary, stop) = (Arc::clone(&primary), Arc::clone(&stop));
            tokio::spawn(async move {
                while !stop.load(Ordering::Relaxed) {
                    let _ = primary.post(BTreeMap::from([("kind".to_string(), "steady".to_string())]), Bytes::new()).await;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
        };
        // Probes every 900 ms: a fixed threshold of 4 syncs about every 3.6 s (five in 20 s); the
        // backed-off one at 3.6 s, then 7.2 s later, then 14.4 s after that.
        tokio::time::sleep(Duration::from_secs(20)).await;
        stop.store(true, Ordering::Relaxed);
        let _ = writer.await;
        let syncs = mirror.mirror_digest_resyncs();
        assert!(syncs >= 1, "replication in flight made the probes mismatch");
        assert!(syncs <= 3, "{syncs} full syncs in 20 s that repaired nothing: the probe did not back off");
        mirror.shutdown().await;
        primary.shutdown().await;
    }
}
