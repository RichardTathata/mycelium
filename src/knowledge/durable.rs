//! **Durable knowledge stores** (Boundary H item K3a) —
//! [`docs/design/knowledge-validity.md`](../../../docs/design/knowledge-validity.md) §4.
//!
//! K1 verified records on storage and K2 verified heads by ancestry, but both kept their state in
//! memory, so a restart lost the records *and* reset rollback protection: a reader that has forgotten
//! its checkpoint accepts a rolled-back head as the first it has ever seen. This module makes both
//! durable.
//!
//! # Composition, not a new primitive
//!
//! Both stores sit on the substrate's existing node-local journal (`crate::agent::journal`):
//! append-only, length-prefixed, **fsynced before it acknowledges**, never gossiped, and already
//! admitted by the replay-seam gate. This module adds no filesystem call of its own, so it adds no
//! site to `scripts/sim-seams-baseline.txt` and no lock to the lock-order table.
//!
//! # Persist, then apply — never the other way round
//!
//! Each write is two-phase. The in-memory core **decides** what it would do without doing it
//! ([`KnowledgeStore::verify_signed`], `HeadCheckpoints::evaluate`). The decision is appended to
//! the journal and **awaited until it is fsynced**. Only then is it applied in memory. A journal
//! that refuses, fails or loses its acknowledgement leaves memory unchanged, and the caller is told
//! ([`PutRefusal::NotPersisted`], [`HeadVerdict::CheckpointNotPersisted`]).
//!
//! A *lost* acknowledgement means the entry may be on disk after all. That is safe: everything
//! appended was already verified, so finding it after a restart is an entry the reader would have
//! accepted anyway.
//!
//! # Reopening fails closed
//!
//! [`open`](DurableHeadCheckpoints::open) replays the journal. An entry that does not decode, or
//! carries an unknown version, **refuses the open**. Starting with partial or empty state would
//! silently reset what a restart must preserve. A missing journal is a fresh reader, not an error.
//!
//! Records are restored with the attribution they were verified under. Authenticity is historical,
//! and present eligibility is re-derived at every read (K1b), so a node that restarts before it has
//! relearned its peers' keys still holds what it verified, and counts it only once it can check it
//! again.
//!
//! # Five-part statement
//!
//! *Guarantee:* a record `put_signed` accepts, and a checkpoint `offer` advances, is on this node's
//! disk, fsynced, before the call reports it, and is present after the node reopens the same
//! journal. *Assumptions:* the journal's (durable local storage, `fsync` meaning what the filesystem
//! says). *Enforcing component:* the two-phase methods here, over `Journal::append`. *Failure
//! behaviour:* `NotPersisted` / `CheckpointNotPersisted` with memory unchanged; an unreadable
//! journal refuses to open. *Detecting tests:* this module's `tests`, covering restart, rollback
//! after restart, corrupt journals, and a missing journal.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::heads::{
    Checkpoint, CheckpointStore, Evaluation, ForkRecord, HeadCheckpoints, HeadVerdict, SignedHead,
};
use super::issuer::{IssuerPath, MemberKeySource, TrustedExternalIssuers};
use super::store::{Attribution, KnowledgeStore, PutRefusal, SignedRecord};
use super::IssuerId;
use crate::agent::journal::{read_journal, Journal};

/// The replay-seam stream for the durable record journal. Distinct per journal, as the journal
/// module requires.
pub const RECORDS_STREAM: &str = "knowledge/records";
/// The replay-seam stream for the durable head-checkpoint journal.
pub const HEADS_STREAM: &str = "knowledge/heads";

const ENTRY_VERSION: u32 = 1;

/// Why a durable store refused to open.
#[non_exhaustive]
#[derive(Debug)]
pub enum DurableOpenError {
    /// The journal exists and could not be read or opened.
    Io(std::io::Error),
    /// An entry does not decode, or has an unknown version. Opening with partial state would reset
    /// what the journal exists to keep, so the store does not open.
    Unreadable {
        /// Which entry, counting from zero.
        entry: usize,
        /// What was wrong.
        why: String,
    },
}

impl std::fmt::Display for DurableOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "knowledge journal could not be opened: {e}"),
            Self::Unreadable { entry, why } => {
                write!(f, "knowledge journal entry {entry} is unreadable: {why}")
            }
        }
    }
}

impl std::error::Error for DurableOpenError {}

/// Every entry in a journal that exists, or none for a journal that does not yet.
fn existing_entries(path: &Path) -> Result<Vec<Vec<u8>>, DurableOpenError> {
    match read_journal(path) {
        Ok(entries) => Ok(entries),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(DurableOpenError::Io(e)),
    }
}

fn decode<T: for<'de> Deserialize<'de>>(entry: usize, bytes: &[u8]) -> Result<T, DurableOpenError> {
    serde_json::from_slice(bytes).map_err(|e| DurableOpenError::Unreadable { entry, why: e.to_string() })
}

// ── heads ────────────────────────────────────────────────────────────────────────────────────

/// One journal entry: a stream's checkpoint moved to here.
#[derive(Serialize, Deserialize)]
struct HeadEntry {
    version: u32,
    issuer: IssuerId,
    stream: String,
    checkpoint: Checkpoint,
}

/// A [`CheckpointStore`] that is never asked to persist: the durable wrapper persists through the
/// journal itself, between evaluate and apply.
struct JournalBacked;

impl CheckpointStore for JournalBacked {
    fn load(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
    fn persist(&self, _: &[u8]) -> Result<(), String> {
        Err("persistence goes through the durable wrapper's journal".to_string())
    }
}

/// [`HeadCheckpoints`] whose every advance is fsynced to a journal before it is reported, and which
/// reopens to the same checkpoints.
pub struct DurableHeadCheckpoints {
    inner: HeadCheckpoints<JournalBacked>,
    journal: Arc<Journal>,
}

impl DurableHeadCheckpoints {
    /// Open the journal at `path`, replaying every checkpoint it holds. Refuses an unreadable
    /// journal. Must run inside a Tokio runtime: the journal spawns its writer.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DurableOpenError> {
        let path = path.as_ref();
        let mut checkpoints: BTreeMap<(IssuerId, String), Checkpoint> = BTreeMap::new();
        for (i, bytes) in existing_entries(path)?.iter().enumerate() {
            let e: HeadEntry = decode(i, bytes)?;
            if e.version != ENTRY_VERSION {
                return Err(DurableOpenError::Unreadable { entry: i, why: format!("version {}", e.version) });
            }
            // Later entries are later advances: the fold keeps the last one per stream.
            checkpoints.insert((e.issuer, e.stream), e.checkpoint);
        }
        let journal = Journal::open(path, HEADS_STREAM).map_err(DurableOpenError::Io)?;
        Ok(Self { inner: HeadCheckpoints::from_checkpoints(JournalBacked, checkpoints), journal })
    }

    /// Offer a head (see `HeadCheckpoints::offer`). An advance is fsynced before it is reported.
    pub async fn offer(
        &mut self,
        offered: &SignedHead,
        intermediates: &[SignedHead],
        members: &impl MemberKeySource,
        external: &TrustedExternalIssuers,
    ) -> HeadVerdict {
        match self.inner.evaluate(offered, intermediates, members, external) {
            Evaluation::Final(verdict) => verdict,
            Evaluation::Fork(fork, verdict) => {
                self.inner.record_fork(fork);
                verdict
            }
            Evaluation::Advance(pending) => {
                let entry = HeadEntry {
                    version: ENTRY_VERSION,
                    issuer: pending.key.0.clone(),
                    stream: pending.key.1.clone(),
                    checkpoint: pending.next,
                };
                let bytes = match serde_json::to_vec(&entry) {
                    Ok(b) => b,
                    Err(e) => return HeadVerdict::CheckpointNotPersisted(e.to_string()),
                };
                if let Err(e) = self.journal.append(bytes).await {
                    return HeadVerdict::CheckpointNotPersisted(e.to_string());
                }
                self.inner.apply(pending)
            }
        }
    }

    /// The checkpoint held for `(issuer, stream)`.
    pub fn checkpoint(&self, issuer: &IssuerId, stream: &str) -> Option<Checkpoint> {
        self.inner.checkpoint(issuer, stream)
    }

    /// Forks seen since this store was opened. Held in memory only.
    pub fn forks(&self) -> &[ForkRecord] {
        self.inner.forks()
    }
}

// ── records ──────────────────────────────────────────────────────────────────────────────────

/// How an attribution is written down. `IssuerPath` holds a `NodeId`, which is kept as its string
/// form here, the same form the member issuer uses.
#[derive(Serialize, Deserialize)]
struct PersistedAttribution {
    member: Option<String>,
    key: [u8; 32],
    revoked_at_storage: bool,
}

/// One journal entry: a verified record, its signature, and what it was verified as.
#[derive(Serialize, Deserialize)]
struct RecordEntry {
    version: u32,
    signed: SignedRecord,
    attribution: PersistedAttribution,
}

fn persist_attribution(a: &Attribution) -> Option<PersistedAttribution> {
    match a {
        Attribution::Verified { path, key, revoked_at_storage } => Some(PersistedAttribution {
            member: match path {
                IssuerPath::Member(n) => Some(n.to_string()),
                IssuerPath::External => None,
            },
            key: *key,
            revoked_at_storage: *revoked_at_storage,
        }),
        Attribution::Unchecked => None,
    }
}

fn restore_attribution(entry: usize, p: PersistedAttribution) -> Result<Attribution, DurableOpenError> {
    let path = match p.member {
        Some(n) => IssuerPath::Member(n.parse().map_err(|_| DurableOpenError::Unreadable {
            entry,
            why: format!("member `{n}` is not a node id"),
        })?),
        None => IssuerPath::External,
    };
    Ok(Attribution::Verified { path, key: p.key, revoked_at_storage: p.revoked_at_storage })
}

/// A [`KnowledgeStore`] whose every verified record is fsynced to a journal before it is stored, and
/// which reopens holding the same records with the same attributions.
///
/// Only [`put_signed`](Self::put_signed) writes. Unchecked records ([`KnowledgeStore::put`]) have no
/// durable form: a record nobody verified is not worth keeping across a restart.
pub struct DurableKnowledgeStore {
    store: KnowledgeStore,
    journal: Arc<Journal>,
}

impl DurableKnowledgeStore {
    /// Open the journal at `path`, restoring every record it holds. Refuses an unreadable journal.
    /// Must run inside a Tokio runtime: the journal spawns its writer.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DurableOpenError> {
        let path = path.as_ref();
        let mut store = KnowledgeStore::new();
        for (i, bytes) in existing_entries(path)?.iter().enumerate() {
            let e: RecordEntry = decode(i, bytes)?;
            if e.version != ENTRY_VERSION {
                return Err(DurableOpenError::Unreadable { entry: i, why: format!("version {}", e.version) });
            }
            let attribution = restore_attribution(i, e.attribution)?;
            store.insert_verified(e.signed, attribution);
        }
        let journal = Journal::open(path, RECORDS_STREAM).map_err(DurableOpenError::Io)?;
        Ok(Self { store, journal })
    }

    /// Verify, persist, then store. See [`KnowledgeStore::put_signed`] for what is verified.
    pub async fn put_signed(
        &mut self,
        signed: SignedRecord,
        members: &impl MemberKeySource,
        external: &TrustedExternalIssuers,
    ) -> Result<Attribution, PutRefusal> {
        let attribution = match self.store.verify_signed(&signed, members, external) {
            Ok(a) => a,
            Err(refusal) => {
                self.store.note_refusal(&refusal);
                return Err(refusal);
            }
        };
        // Already held and verified: nothing new to persist.
        if self.store.attribution(signed.record.id()).is_some_and(Attribution::is_verified) {
            return Ok(attribution);
        }
        let Some(persisted) = persist_attribution(&attribution) else {
            // verify_signed only ever returns Verified; refuse rather than write something else.
            let refusal = PutRefusal::NotPersisted("not a verified attribution".to_string());
            self.store.note_refusal(&refusal);
            return Err(refusal);
        };
        let entry = RecordEntry { version: ENTRY_VERSION, signed: signed.clone(), attribution: persisted };
        let appended = match serde_json::to_vec(&entry) {
            Ok(bytes) => self.journal.append(bytes).await.map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        if let Err(e) = appended {
            let refusal = PutRefusal::NotPersisted(e);
            self.store.note_refusal(&refusal);
            return Err(refusal);
        }
        Ok(self.store.insert_verified(signed, attribution))
    }

    /// The store, for reading and resolution.
    pub fn store(&self) -> &KnowledgeStore {
        &self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::issuer::MemberKeys;
    use crate::knowledge::resolution::{classify_eligible, ReaderPolicy, ReleaseId, UncheckedRule};
    use crate::knowledge::store::Head;
    use crate::knowledge::{KnowledgeRecord, Link, LinkKind, RecordId, RecordKind};
    use crate::node_id::NodeId;
    use ed25519_dalek::SigningKey;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("myc-k3a-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("journal")
    }

    struct F {
        sk: SigningKey,
        node: NodeId,
        members: HashMap<NodeId, MemberKeys>,
        ext: TrustedExternalIssuers,
    }

    fn fixture() -> F {
        let sk = SigningKey::from_bytes(&[4u8; 32]);
        let node = NodeId::new("127.0.0.1", 7401).unwrap();
        let members = HashMap::from([(
            node.clone(),
            MemberKeys { retained: vec![sk.verifying_key().to_bytes()], ..Default::default() },
        )]);
        F { sk, node, members, ext: TrustedExternalIssuers::new() }
    }

    impl F {
        fn issuer(&self) -> IssuerId {
            IssuerId::for_node(&self.node)
        }

        fn head(&self, seq: u64, prev: Option<&SignedHead>) -> SignedHead {
            let head = Head {
                issuer: self.issuer(),
                stream: "s".into(),
                record: RecordId { issuer: self.issuer(), digest: [seq as u8; 32] },
                seq,
                prev: prev.map(|p| p.head.digest()),
            };
            let signature = mycelium_core::tls::sign_bytes(&self.sk, &head.canonical_bytes()).to_vec();
            SignedHead { head, signature }
        }

        fn support(&self, release: &ReleaseId) -> SignedRecord {
            let record = KnowledgeRecord::new(
                self.issuer(),
                RecordKind::Assessment,
                1_000,
                release.subject(),
                b"j".to_vec(),
                vec![Link {
                    kind: LinkKind::Supports,
                    target: RecordId { issuer: IssuerId::new("provider").unwrap(), digest: [7; 32] },
                }],
            )
            .unwrap();
            let signature = mycelium_core::tls::sign_bytes(&self.sk, &record.canonical_bytes()).to_vec();
            SignedRecord { record, signature }
        }
    }

    /// **The caveat K2 left open, closed.** A checkpoint survives a real reopen of the journal, and
    /// the reopened reader still refuses a rolled-back head.
    #[tokio::test]
    async fn head_checkpoints_survive_a_reopen_and_keep_refusing_rollback() {
        let f = fixture();
        let path = temp_path("heads");
        let h1 = f.head(1, None);
        let h2 = f.head(2, Some(&h1));
        {
            let mut r = DurableHeadCheckpoints::open(&path).unwrap();
            assert_eq!(r.offer(&h1, &[], &f.members, &f.ext).await, HeadVerdict::Advanced { from: None, to: 1 });
            assert_eq!(r.offer(&h2, &[], &f.members, &f.ext).await, HeadVerdict::Advanced { from: Some(1), to: 2 });
        }
        let mut reopened = DurableHeadCheckpoints::open(&path).unwrap();
        assert_eq!(reopened.checkpoint(&f.issuer(), "s").unwrap().seq, 2);
        assert_eq!(
            reopened.offer(&h1, &[], &f.members, &f.ext).await,
            HeadVerdict::StaleHead { held: 2, offered: 1 }
        );
    }

    /// Verified records survive a reopen with the attribution they were verified under, and still
    /// count at resolution.
    #[tokio::test]
    async fn verified_records_survive_a_reopen_with_their_attribution() {
        let f = fixture();
        let path = temp_path("records");
        let release = ReleaseId::new("svc", "1.0");
        let signed = f.support(&release);
        let id = signed.record.id().clone();
        {
            let mut s = DurableKnowledgeStore::open(&path).unwrap();
            assert!(s.put_signed(signed, &f.members, &f.ext).await.unwrap().is_verified());
        }
        let reopened = DurableKnowledgeStore::open(&path).unwrap();
        assert!(reopened.store().attribution(&id).unwrap().is_verified());
        assert!(reopened.store().signature(&id).is_some());
        let strict = ReaderPolicy { unchecked: UncheckedRule::Exclude, ..Default::default() };
        let c = classify_eligible(
            reopened.store(),
            &release,
            &IssuerId::new("provider").unwrap(),
            &strict,
            2_000,
            &f.members,
            &f.ext,
        );
        assert!(c.verdict.is_accepted(), "{c:?}");
    }

    /// A refused record is not written: it is absent after a reopen.
    #[tokio::test]
    async fn a_refused_record_is_not_persisted() {
        let f = fixture();
        let path = temp_path("refused");
        let mut forged = f.support(&ReleaseId::new("svc", "1.0"));
        forged.signature[0] ^= 0xff;
        let id = forged.record.id().clone();
        {
            let mut s = DurableKnowledgeStore::open(&path).unwrap();
            assert!(s.put_signed(forged, &f.members, &f.ext).await.is_err());
        }
        assert!(DurableKnowledgeStore::open(&path).unwrap().store().get(&id).is_none());
    }

    /// **Fails closed.** A journal holding an entry that is not a checkpoint refuses to open, rather
    /// than opening empty and resetting rollback protection.
    #[tokio::test]
    async fn an_unreadable_journal_refuses_to_open() {
        let path = temp_path("corrupt");
        {
            let j = Journal::open(&path, "knowledge/test-corrupt").unwrap();
            j.append(b"not a checkpoint".to_vec()).await.unwrap();
        }
        assert!(matches!(
            DurableHeadCheckpoints::open(&path),
            Err(DurableOpenError::Unreadable { entry: 0, .. })
        ));
        assert!(matches!(
            DurableKnowledgeStore::open(&path),
            Err(DurableOpenError::Unreadable { entry: 0, .. })
        ));
    }

    /// A journal that does not exist yet is a fresh reader, not an error.
    #[tokio::test]
    async fn a_missing_journal_opens_empty() {
        let path = temp_path("fresh");
        let r = DurableHeadCheckpoints::open(&path).unwrap();
        assert!(r.checkpoint(&IssuerId::new("x").unwrap(), "s").is_none());
    }
}
