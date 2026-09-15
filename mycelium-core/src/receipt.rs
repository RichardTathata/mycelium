//! Receipts — what an acknowledgement proves, by rung (v3 contracts axis item 1, PR 2).
//!
//! The record is [`docs/design/contracts-receipts.md`](../../../docs/design/contracts-receipts.md);
//! this module is its vocabulary in code. Four receipts, never conflated and never inferred from
//! one another:
//!
//! | Receipt | Establishes | This PR |
//! |---|---|---|
//! | **local application** | the operation was applied to this node's store, or superseded under LWW | [`LocalApplication`], returned |
//! | **local sync** | *this exact operation* crossed this node's persistence barrier | [`LocalDurability`], returned |
//! | **replica sync** | named, distinct peers persisted *this exact operation* | [`ReplicaSync`] — the vocabulary; PR 4a/4b establish it |
//! | **destination commit** | a destination committed the business change and its dedup result in one transaction | [`DestinationCommit`] — the vocabulary; PR 5 establishes it |
//!
//! **A receipt names its rung and nothing above it.** `Applied` is not `OnDisk`; `OnDisk` on one
//! node is not replica sync; three replicas are not a destination commit. Nothing here infers a
//! higher rung from a lower one.
//!
//! **Identity.** Every operation carries a caller-minted [`OperationId`], stable across retries and
//! worker replacement, and each delivery attempt an [`AttemptId`]. These are *the* identities —
//! the AE action envelope binds them and the `GatewayCaller` context attributes them; nothing else
//! mints a second scheme.
//!
//! **A timeout is not a negative.** Where an operation's fate is unknown the answer is
//! [`ReceiptError::DeliveryUnknown`], carrying the rungs that *were* established. No verb here
//! returns "nothing happened", because no verb here can know that.

use crate::node_id::NodeId;
use std::sync::Arc;

// ── Identities ───────────────────────────────────────────────────────────────

/// A caller-minted identity for a logical operation, assigned **before dispatch** and **stable
/// across retries**, worker replacement and gateway hops. The key of every receipt, and of
/// destination-side dedup.
///
/// It is a *correlation* identity, never authority: a client may supply one (that is the point —
/// its retries must be recognisable), while who the caller *is* comes only from the verified
/// caller context.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OperationId(Arc<str>);

impl OperationId {
    /// Adopt a caller-supplied identity.
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        Self(id.into())
    }

    /// Mint one for a caller that has none: `{node}/{random}`. Unique, but **not** stable across
    /// processes — a caller whose retries must be recognised supplies its own.
    pub fn generate(node: &NodeId) -> Self {
        Self(Arc::from(format!("{node}/{:016x}", fastrand::u64(..)).as_str()))
    }

    /// The identity as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OperationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One delivery attempt of an [`OperationId`] — what distinguishes "the second try was acked" from
/// "the first was".
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AttemptId(Arc<str>);

impl AttemptId {
    /// The `n`-th attempt of `op`, rendered `{operation}#{n}`.
    pub fn of(op: &OperationId, n: u32) -> Self {
        Self(Arc::from(format!("{op}#{n}").as_str()))
    }

    /// The attempt as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AttemptId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ── Rung 1: local application ────────────────────────────────────────────────

/// What became of an operation at this node's store — the **local application** receipt.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalApplication {
    /// This operation's value is the one the store now holds.
    Applied,
    /// A newer value already held the key, so LWW kept it. The operation is not lost or failed;
    /// it is *superseded*, and a reader sees the newer value.
    Superseded,
}

// ── Rung 2: local sync ───────────────────────────────────────────────────────

/// Whether **this exact operation** crossed this node's persistence barrier — the **local sync**
/// receipt.
///
/// Read [`Failed`](Self::Failed) precisely: it means *durability was not established*, **not**
/// that the record is absent. The WAL writes the record before it syncs, so after a failure the
/// bytes may or may not be on disk and a later replay may restore them. Nothing is promised;
/// nothing is denied either.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalDurability {
    /// The record was written **and synced**: a forced `fdatasync` returned, or the node's
    /// `SyncMode::Flush` syncs every append. The one state that establishes durability.
    OnDisk,
    /// The WAL accepted the record, but this node's [`SyncMode`](crate::config::SyncMode) does not
    /// force a sync per append (`Async`, `Os`), so the bytes sit in the OS page cache: they
    /// survive a process crash and are lost to a power failure until the next sync or snapshot.
    ///
    /// *This state was missing from the first draft of the contract record*, which assumed every
    /// write was either forced to disk or failed — true only under `Flush` or an explicit
    /// `append_sync`. A receipt that said `OnDisk` here would have claimed a durability the node
    /// never established (found while implementing PR 2, 2026-09-15). PR 3's required-sync write is
    /// how a caller demands `OnDisk` instead.
    Buffered,
    /// Durability was **not established**: the writer was gone, or the write or sync returned an
    /// error. Never read as "the record is absent".
    Failed(String),
    /// No persistence is configured on this node, so nothing was promised and nothing is claimed.
    NotConfigured,
}

impl LocalDurability {
    /// Does this receipt establish that the operation is on disk? Only [`OnDisk`](Self::OnDisk)
    /// does — `Buffered` and `NotConfigured` promise nothing, and `Failed` denies nothing.
    pub fn is_durable(&self) -> bool {
        matches!(self, LocalDurability::OnDisk)
    }
}

// ── Rung 3: replica sync (vocabulary; established by PR 4a/4b) ───────────────

/// Which **named, distinct peers** persisted this exact operation — the **replica sync** receipt.
///
/// The origin is never counted. Today no verb returns a populated one: `set_with_min_acks` counts
/// *propagation* (any update at or after the write's timestamp, from any peer), which is evidence
/// that something newer exists, not that a named peer holds *this* payload. PR 4a makes the count
/// exact-identity; PR 4b adds the persisted-by-peer protocol that fills `persisted_by`.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplicaSync {
    /// Peers that reported persisting **this exact operation**.
    pub persisted_by: Vec<NodeId>,
    /// Peers that were asked and did not report — *unknown*, not "did not persist".
    pub missing: Vec<NodeId>,
}

impl ReplicaSync {
    /// A replica-sync receipt naming who persisted and who did not answer.
    pub fn new(persisted_by: Vec<NodeId>, missing: Vec<NodeId>) -> Self {
        Self { persisted_by, missing }
    }
}

// ── Rung 4: destination commit (vocabulary; established by PR 5) ─────────────

/// Whether a destination's transaction was the first to carry this operation, or a replay of one
/// it had already committed.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DedupOutcome {
    /// The destination had not seen this `operation_id`; this transaction applied it.
    Fresh,
    /// The destination had already committed this `operation_id` and applied nothing again.
    Replayed,
}

/// A destination committed the business change **and** its dedup result in one transaction — the
/// **destination commit** receipt, the only rung that establishes an exactly-once *effect*, and
/// the only one the substrate never provides on its own.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DestinationCommit {
    /// Which destination committed — its own identity, not a node id.
    pub destination: String,
    /// Whether this attempt applied the change or found it already applied.
    pub dedup: DedupOutcome,
}

impl DestinationCommit {
    /// A destination-commit receipt.
    pub fn new(destination: impl Into<String>, dedup: DedupOutcome) -> Self {
        Self { destination: destination.into(), dedup }
    }
}

// ── The write receipt ────────────────────────────────────────────────────────

/// What a KV write established, rung by rung.
///
/// Construct receipts through [`WriteReceipt::new`] and the `with_*` builders: the type is
/// `#[non_exhaustive]` so later rungs are additive, and a type an external caller cannot build is
/// a type it cannot test against (the lesson from the AE seam's review, 2026-09-15).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteReceipt {
    /// The logical operation this write belongs to.
    pub operation_id: OperationId,
    /// This delivery attempt.
    pub attempt_id: AttemptId,
    /// The key written.
    pub key: Arc<str>,
    /// A stable hash of the operation's content, used to detect *same identity, different content*
    /// on a retry. A divergence detector, **not** a security binding: it is not a cryptographic
    /// digest and does not defend against a chosen collision (the AE envelope's `sha256` is the
    /// one that must).
    pub content_hash: u64,
    /// The HLC this operation was stamped with. **A retry must reuse it** rather than tick a fresh
    /// one, or the same operation would rank differently under LWW on each attempt; that is what
    /// [`KvHandle::retry_with_receipt`](crate::kv_handle::KvHandle::retry_with_receipt) does.
    pub stamp: u64,
    /// Rung 1.
    pub application: LocalApplication,
    /// Rung 2.
    pub local_durability: LocalDurability,
    /// Rung 3 — empty until PR 4a/4b establish it.
    pub replica_sync: ReplicaSync,
    /// Was the update handed to the gossip fan-out? Propagation is not a rung: it establishes
    /// nothing about any peer's state. `false` means anti-entropy will carry it later.
    pub queued_for_gossip: bool,
}

impl WriteReceipt {
    /// A receipt for `operation_id`/`attempt_id` on `key`.
    pub fn new(
        operation_id: OperationId,
        attempt_id: AttemptId,
        key: Arc<str>,
        content_hash: u64,
        stamp: u64,
        application: LocalApplication,
    ) -> Self {
        Self {
            operation_id,
            attempt_id,
            key,
            content_hash,
            stamp,
            application,
            local_durability: LocalDurability::NotConfigured,
            replica_sync: ReplicaSync::default(),
            queued_for_gossip: false,
        }
    }

    /// Set the local-sync rung.
    pub fn with_local_durability(mut self, d: LocalDurability) -> Self {
        self.local_durability = d;
        self
    }

    /// Set the replica-sync rung.
    pub fn with_replica_sync(mut self, r: ReplicaSync) -> Self {
        self.replica_sync = r;
        self
    }

    /// Record whether the update was queued for gossip.
    pub fn queued(mut self, queued: bool) -> Self {
        self.queued_for_gossip = queued;
        self
    }
}

// ── The commit receipt ───────────────────────────────────────────────────────

/// What a consensus commit established on **this** node.
///
/// This is the receipt-shaped answer that replaces reading `ConsensusResult::Committed`'s
/// `persisted: bool`. It is a **new type returned by a new verb**, not a new field on that
/// variant: the variant's fields are not `#[non_exhaustive]`, so adding one would break every
/// exhaustive destructure and construction — not additive by Rust's rules, whatever the intent
/// (D24, corrected after review 2026-09-14).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitReceipt {
    /// The slot committed.
    pub slot: Arc<str>,
    /// The value that won.
    pub value: bytes::Bytes,
    /// The ballot that carried it.
    pub ballot: u64,
    /// Whether the committed record crossed **this node's** persistence barrier. Cluster-wide
    /// agreement is not local durability, and neither implies the other.
    pub local_durability: LocalDurability,
}

impl CommitReceipt {
    /// A commit receipt.
    pub fn new(slot: Arc<str>, value: bytes::Bytes, ballot: u64, local_durability: LocalDurability) -> Self {
        Self { slot, value, ballot, local_durability }
    }
}

/// Why a proposal produced no commit receipt.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitError {
    /// No quorum answered before the deadline. **Not** "the value was not committed": another
    /// node may have committed it, and a later read may find it.
    DeliveryUnknown {
        /// The slot whose fate is unknown.
        slot: Arc<str>,
        /// How many ballots were attempted.
        ballots_tried: u32,
    },
    /// A different value already holds the slot; this proposal lost.
    Superseded {
        /// The slot.
        slot: Arc<str>,
        /// The ballot observed to have superseded it.
        ballot: u64,
    },
    /// The group's topology requirement was not met, so no commit was attempted.
    TopologyUnsatisfied {
        /// The slot.
        slot: Arc<str>,
        /// How many distinct failure domains answered.
        distinct: usize,
        /// How many were required.
        required: usize,
    },
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommitError::DeliveryUnknown { slot, ballots_tried } => write!(
                f,
                "delivery unknown for slot {slot} after {ballots_tried} ballots —                  the value may or may not have committed elsewhere"
            ),
            CommitError::Superseded { slot, ballot } => {
                write!(f, "slot {slot} was superseded at ballot {ballot}")
            }
            CommitError::TopologyUnsatisfied { slot, distinct, required } => write!(
                f,
                "slot {slot}: {distinct} distinct failure domains answered, {required} required"
            ),
        }
    }
}

impl std::error::Error for CommitError {}

// ── Failures ─────────────────────────────────────────────────────────────────

/// Why an operation did not produce the receipt the caller asked for.
///
/// Note what is **not** here: a variant meaning "nothing happened". No verb on this path can
/// establish that.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiptError {
    /// The same [`OperationId`] was retried with **different content**. The operation is not
    /// re-stamped and nothing is written: an identity that meant two things would make every
    /// receipt about it ambiguous.
    Conflict {
        /// The operation whose content changed.
        operation_id: OperationId,
        /// The content hash the prior attempt carried.
        expected: u64,
        /// The content hash of this attempt.
        found: u64,
    },
    /// The operation's fate is **unknown**: a deadline elapsed with no answer. The rungs that were
    /// established are carried; the rest is unknown, not negative.
    DeliveryUnknown {
        /// What *was* established before the answer stopped coming.
        established: Box<WriteReceipt>,
        /// What the caller was waiting for.
        awaiting: &'static str,
    },
    /// The write was refused before anything was applied — an oversized value, a closed shard.
    Rejected(String),
}

impl std::fmt::Display for ReceiptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReceiptError::Conflict { operation_id, expected, found } => write!(
                f,
                "operation {operation_id} was retried with different content \
                 (content hash {expected:016x} then {found:016x}); nothing was written"
            ),
            ReceiptError::DeliveryUnknown { awaiting, established } => write!(
                f,
                "delivery unknown while awaiting {awaiting}; established so far: applied={:?}, durability={:?}",
                established.application, established.local_durability
            ),
            ReceiptError::Rejected(why) => write!(f, "write rejected: {why}"),
        }
    }
}

impl std::error::Error for ReceiptError {}

/// A stable content hash for conflict detection (see [`WriteReceipt::content_hash`]).
///
/// Fixed-seed so it is identical across processes and runs — a receipt from one node must be
/// comparable with a retry on another.
pub fn content_hash(key: &str, value: &[u8], is_tombstone: bool) -> u64 {
    use std::hash::{BuildHasher, Hash, Hasher};
    static SEED: std::sync::OnceLock<ahash::RandomState> = std::sync::OnceLock::new();
    let state = SEED.get_or_init(|| ahash::RandomState::with_seeds(0x5eed, 0xc0ffee, 0xfeed, 0xface));
    let mut h = state.build_hasher();
    key.hash(&mut h);
    value.hash(&mut h);
    is_tombstone.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_receipt_names_its_rung_and_nothing_above_it() {
        let op = OperationId::new("op-1");
        let r = WriteReceipt::new(
            op.clone(),
            AttemptId::of(&op, 1),
            Arc::from("k"),
            content_hash("k", b"v", false),
            42,
            LocalApplication::Applied,
        );
        // Applied says nothing about disk, and the default durability claims nothing.
        assert_eq!(r.application, LocalApplication::Applied);
        assert_eq!(r.local_durability, LocalDurability::NotConfigured);
        assert!(!r.local_durability.is_durable());
        // Nor about any peer.
        assert!(r.replica_sync.persisted_by.is_empty());
        // Only OnDisk establishes durability — Buffered and Failed do not.
        assert!(LocalDurability::OnDisk.is_durable());
        assert!(!LocalDurability::Buffered.is_durable());
        assert!(!LocalDurability::Failed("writer gone".into()).is_durable());
        assert!(!LocalDurability::NotConfigured.is_durable());
    }

    #[test]
    fn attempts_are_distinguishable_and_the_operation_is_stable() {
        let op = OperationId::new("payment-7");
        let a1 = AttemptId::of(&op, 1);
        let a2 = AttemptId::of(&op, 2);
        assert_ne!(a1, a2, "attempts differ");
        assert_eq!(a1.as_str(), "payment-7#1");
        assert_eq!(op.as_str(), "payment-7", "the operation identity is the caller's, unchanged");
    }

    #[test]
    fn the_content_hash_is_stable_and_separates_content() {
        assert_eq!(content_hash("k", b"v", false), content_hash("k", b"v", false));
        assert_ne!(content_hash("k", b"v", false), content_hash("k", b"w", false));
        assert_ne!(content_hash("k", b"v", false), content_hash("j", b"v", false));
        assert_ne!(content_hash("k", b"", false), content_hash("k", b"", true), "a tombstone is not an empty value");
    }

    #[test]
    fn delivery_unknown_carries_what_was_established() {
        let op = OperationId::new("op-2");
        let established = WriteReceipt::new(
            op.clone(),
            AttemptId::of(&op, 1),
            Arc::from("k"),
            0,
            7,
            LocalApplication::Applied,
        )
        .with_local_durability(LocalDurability::OnDisk);
        let e = ReceiptError::DeliveryUnknown { established: Box::new(established), awaiting: "replica sync" };
        let text = e.to_string();
        assert!(text.contains("delivery unknown"), "{text}");
        assert!(text.contains("OnDisk"), "the rungs established are carried, not discarded: {text}");
    }
}
