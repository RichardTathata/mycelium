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
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
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
    /// A **fresh identity for one dispatch**: `{operation}#{16 random hex}`.
    ///
    /// Each delivery gets its own, because two deliveries can otherwise collide: retrying twice
    /// from the same prior receipt — which happens naturally when a retry's response is lost, or
    /// when replacement workers share the last receipt they saw — would derive the same number
    /// twice and make two distinct deliveries indistinguishable (review, 2026-09-15). A caller that
    /// wants deterministic numbering mints its own with [`of`](Self::of) and passes it to the
    /// `*_as` verbs.
    pub fn fresh(op: &OperationId) -> Self {
        Self(Arc::from(format!("{op}#{:016x}", fastrand::u64(..)).as_str()))
    }

    /// The `n`-th attempt of `op`, rendered `{operation}#{n}` — for a caller that numbers its own
    /// attempts and can guarantee it does not reuse a number.
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

// ── Prepared operations ──────────────────────────────────────────────────────

/// An operation **stamped before dispatch** — identity, target, content binding and HLC — so the
/// caller can retry it safely without needing the node, or its own earlier receipt, to remember
/// anything.
///
/// This is what makes a retry safe after a **lost acknowledgement**. Re-issuing an ordinary write
/// with the same [`OperationId`] mints a *fresh* HLC, so a retry that arrives after something newer
/// took the key would outrank and silently undo it — the hazard D11 exists to prevent. Committing
/// the same `PreparedWrite` again re-submits the original stamp, so a late retry loses LWW and is
/// reported [`LocalApplication::Superseded`] instead (review of PR 3, 2026-09-15).
///
/// It is `Serialize`/`Deserialize` on purpose: a caller that must survive **its own** restart —
/// the case where a receipt is lost — persists this token beside whatever made it decide to write.
/// Small and self-contained: no node state, nothing to expire, nothing to evict.
///
/// A prepared write binds its content. Committing it with different bytes is a
/// [`ReceiptError::Conflict`]; genuinely different content is a different operation and needs its
/// own token.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PreparedWrite {
    /// The logical operation.
    pub operation_id: OperationId,
    /// The key this operation writes.
    pub key: Arc<str>,
    /// The content binding, from [`content_hash`].
    pub content_hash: u64,
    /// The HLC allocated at preparation — reused by **every** attempt.
    pub stamp: u64,
}

impl PreparedWrite {
    /// Build a prepared write directly (the usual route is
    /// [`KvHandle::prepare_write`](crate::kv_handle::KvHandle::prepare_write), which allocates the
    /// stamp from the node's HLC).
    pub fn new(operation_id: OperationId, key: Arc<str>, content_hash: u64, stamp: u64) -> Self {
        Self { operation_id, key, content_hash, stamp }
    }
}

// ── Rung 1: local application ────────────────────────────────────────────────

/// What became of an operation at this node's store — the **local application** receipt.
///
/// Four outcomes, because the store's "nothing changed" covers three different truths and a
/// receipt that called them all `Superseded` would claim a newer value had won when none had
/// (found by review, 2026-09-15).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalApplication {
    /// This operation's value is the one the store now holds, and this write is what put it there.
    Applied,
    /// The store already held **exactly this operation's value at its stamp** — an idempotent
    /// retry. Nothing changed because nothing needed to: the operation *is* current. Distinct from
    /// [`Superseded`](Self::Superseded), where a *different, newer* value won.
    AlreadyCurrent,
    /// A **newer** value already held the key, so LWW kept it. The operation is not lost or
    /// failed; a reader sees the newer value.
    Superseded,
    /// The store **refused** the write: the live-entry cap (`max_store_entries`) was reached. The
    /// key may be absent entirely — nothing newer won, and nothing was applied.
    Refused,
}

impl LocalApplication {
    /// Does the store now hold this operation's value — whether this write put it there or found
    /// it already current?
    pub fn is_current(&self) -> bool {
        matches!(self, LocalApplication::Applied | LocalApplication::AlreadyCurrent)
    }
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
/// The origin is never counted, and today **no verb returns a populated one**.
///
/// `set_with_min_acks` is the nearest thing and it cannot fill this receipt: the origin of a write
/// never observes that write's propagation, because an update's `sender` is its originating node
/// across every hop, fan-out excludes the origin, and anti-entropy re-attributes what it delivers
/// to the receiving node (measured 2026-09-15; `docs/design/contracts-receipts.md` §1a). Since
/// PR 4a its counter at least requires the payload's [`content_hash`] to match, so a newer
/// overwrite is no longer mistaken for evidence. PR 4b adds the persisted-by-peer protocol that
/// fills `persisted_by`.
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
    /// A **required-sync** write did not establish durability, so **this attempt applied nothing
    /// and gossiped nothing** — no reader, subscriber or peer saw the value as a result of the call.
    ///
    /// **What this does not promise.** That the operation can never become visible here. The WAL
    /// writes a record *before* it syncs it, so a sync that fails may leave the bytes in `wal.bin`,
    /// and a later replay would restore them — the value could appear after a restart even though
    /// the call reported failure. The recovery outcome is **unknown**, and making it permanently
    /// non-applied would need machinery the WAL does not have (a two-phase staging area, or a
    /// compensating record that replay honours). Stated rather than implied, after review found the
    /// first cut claiming the stronger property (2026-09-15).
    ///
    /// This is the one place the substrate *prevents* rather than detects, and it is admissible
    /// because the caller asked for it as a contract (posture rule 3(i)): a write that must be
    /// durable is refused rather than applied and reported as undurable.
    /// `persistence_configured` distinguishes *this node was never able to* from *the attempt failed*.
    DurabilityNotEstablished {
        /// Whether persistence is configured at all on this node.
        persistence_configured: bool,
        /// What the WAL reported, or why it could not be asked.
        reason: String,
    },
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
            ReceiptError::DurabilityNotEstablished { persistence_configured: false, .. } => f.write_str(
                "a required-sync write was refused: this node has no persistence configured, so it \
                 cannot establish durability; nothing was applied",
            ),
            ReceiptError::DurabilityNotEstablished { reason, .. } => write!(
                f,
                "a required-sync write did not establish durability ({reason}); this attempt applied \
                 nothing, though a written-but-unsynced record may still replay after a restart"
            ),
        }
    }
}

impl std::error::Error for ReceiptError {}

/// A **specified, portable** content hash for conflict detection (see
/// [`WriteReceipt::content_hash`]).
///
/// **FNV-1a, 64-bit**, over a canonical encoding: the key's UTF-8 bytes, a `0x00` separator, the
/// value's bytes, and a final `0x01`/`0x00` tombstone byte. Length-prefix-free but unambiguous,
/// because the separator cannot occur in the key (a `str` may contain NUL, so the key length is
/// mixed in first — see the encoding below).
///
/// The first cut used `ahash` with fixed seeds, which is **not** an interchange format: aHash
/// documents that its output may differ across versions and CPU features, so the same unchanged
/// operation retried on a different build could hash differently and raise a false
/// [`ReceiptError::Conflict`] (found by review, 2026-09-15). A receipt that travels between nodes
/// needs an algorithm that is specified, not merely seeded. Pinned by golden vectors in this
/// module's tests.
///
/// This is a **divergence detector, not a security binding**: FNV-1a is not collision-resistant and
/// an adversary who chooses both contents can collide it. Where a binding must resist that — the AE
/// action envelope's argument digest — a cryptographic hash is used instead.
pub fn content_hash(key: &str, value: &[u8], is_tombstone: bool) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET_BASIS;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(PRIME);
        }
    };
    // Canonical encoding: key length (8 bytes, big-endian) ‖ key ‖ 0x00 ‖ value ‖ tombstone byte.
    // The length prefix makes ("ab", "c") and ("a", "bc") distinct whatever the bytes contain.
    feed(&(key.len() as u64).to_be_bytes());
    feed(key.as_bytes());
    feed(&[0x00]);
    feed(value);
    feed(&[if is_tombstone { 0x01 } else { 0x00 }]);
    h
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
        // The length prefix keeps a key/value split unambiguous.
        assert_ne!(content_hash("ab", b"c", false), content_hash("a", b"bc", false));
    }

    /// **Golden vectors.** The receipt's hash is an interchange value — a retry may be issued by a
    /// different build on different hardware, and a hash that drifted would raise a false
    /// `Conflict`. These pin FNV-1a/64 over the canonical encoding; a change to either must fail
    /// here and be a deliberate, versioned decision (review, 2026-09-15).
    #[test]
    fn content_hash_golden_vectors() {
        // Computed from the specification: FNV-1a 64 over
        // len(key) as 8 big-endian bytes ‖ key ‖ 0x00 ‖ value ‖ tombstone byte.
        fn expected(key: &str, value: &[u8], tomb: bool) -> u64 {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&(key.len() as u64).to_be_bytes());
            bytes.extend_from_slice(key.as_bytes());
            bytes.push(0x00);
            bytes.extend_from_slice(value);
            bytes.push(if tomb { 0x01 } else { 0x00 });
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for b in bytes {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
            h
        }
        for (k, v, t) in [("", &b""[..], false), ("k", b"v", false), ("k", b"v", true), ("user/1", b"hello world", false)] {
            assert_eq!(content_hash(k, v, t), expected(k, v, t), "key={k:?} value={v:?} tombstone={t}");
        }
        // Fixed literals, so a silent algorithm swap cannot pass by recomputing itself. These were
        // verified against an independent implementation of FNV-1a/64 over the same canonical
        // encoding, not copied from this code's output.
        assert_eq!(content_hash("", b"", false), 0x69d3_07cc_20f6_ef8d);
        assert_eq!(content_hash("k", b"v", false), 0xa498_890a_4e8a_2eaf);
        assert_eq!(content_hash("k", b"v", true), 0xa498_880a_4e8a_2cfc);
        assert_eq!(content_hash("user/1", b"hello world", false), 0x2565_4ee1_ba96_29b0);
    }

    #[test]
    fn an_application_outcome_says_whether_the_value_is_current() {
        assert!(LocalApplication::Applied.is_current());
        assert!(LocalApplication::AlreadyCurrent.is_current(), "an idempotent retry is current, not superseded");
        assert!(!LocalApplication::Superseded.is_current());
        assert!(!LocalApplication::Refused.is_current(), "a capacity refusal is not a supersession");
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
