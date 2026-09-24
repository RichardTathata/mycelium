//! **Signed heads, verified ancestry, durable checkpoints** (Boundary H item K2) —
//! [`docs/design/knowledge-validity.md`](../../../docs/design/knowledge-validity.md) §3.
//!
//! # The gap this closes
//!
//! A stream head (`knowledge/head/{issuer}/{stream}`) was an unsigned pointer with a `seq`, and
//! [`KnowledgeStore::advance_head`](super::store::KnowledgeStore::advance_head) accepted any higher
//! `seq`. That catches a replayed *older* head. It does not catch a **higher** head from a
//! different history: a reader holding head 10 could be handed head 12 from a branch that diverged
//! at 9, and "higher sequence" would have been taken as "newer". Nor was a head authenticated at
//! all.
//!
//! # What a reader now does with an offered head
//!
//! 1. **Authenticate it** through issuer binding's two admissible paths
//!    ([`verify_signed_by`]). A head that fails reads as absent. A head signed under a revoked key is
//!    refused: a head is a *present-tense* claim, and a revoked key has no present standing.
//! 2. **Compare it with the checkpoint** — the latest head this reader has verified *by ancestry* for
//!    the stream:
//!    - lower `seq` → [`HeadVerdict::StaleHead`];
//!    - same `seq`, same head → [`HeadVerdict::AlreadyHeld`]; different head → a **fork**;
//!    - higher `seq` → advance **only** through an unbroken chain of signed heads, linked by
//!      [`Head::prev`], back to the checkpoint.
//! 3. **Report what the chain shows:** a verified extension advances; missing links are
//!    [`HeadVerdict::ContinuityUnavailable`] and the checkpoint stays; a chain that diverges before
//!    the checkpoint is [`HeadVerdict::ForkedStream`], and both heads are retained and reported.
//!
//! # Durable, or it protects nothing across a restart
//!
//! A reader that forgot its checkpoints on restart would accept a rolled-back head as the first it
//! had ever seen. So [`HeadCheckpoints`] **loads** its state from a [`CheckpointStore`] when opened —
//! and refuses to open if that state is unreadable, rather than starting empty — and **persists**
//! every advance *before* reporting it. A persist failure is [`HeadVerdict::CheckpointNotPersisted`]
//! and the checkpoint does not move.
//!
//! This module defines the contract and ships only [`MemoryCheckpointStore`], which is **not
//! durable** and exists for tests. A durable store must go through the filesystem seam
//! (`mycelium_core::sim_seam`, gated by `scripts/check-sim-seams.sh`); it arrives with K3's durable
//! record store. Until then, rollback protection across a restart is exactly as durable as the
//! [`CheckpointStore`] the embedder supplies — said here rather than implied.
//!
//! # Trust on first use, stated
//!
//! The first head a reader sees for a stream has nothing to be checked against, so it becomes the
//! checkpoint once authenticated. Continuity is guaranteed **from the first checkpoint on**, never
//! before it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::issuer::{
    verify_signed_by, Authenticity, MemberKeySource, TrustedExternalIssuers, UnverifiableReason,
};
use super::store::Head;
use super::IssuerId;

/// A head together with its issuer's signature over [`Head::canonical_bytes`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedHead {
    /// The head.
    pub head: Head,
    /// Its issuer's Ed25519 signature over the head's canonical bytes.
    pub signature: Vec<u8>,
}

/// What a reader holds for one stream: the latest head verified by ancestry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Its sequence number.
    pub seq: u64,
    /// Its [`Head::digest`].
    pub digest: [u8; 32],
}

/// A fork: two authenticated, incompatible heads for one stream. Both are kept; neither wins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkRecord {
    /// Whose stream.
    pub issuer: IssuerId,
    /// Which stream.
    pub stream: String,
    /// What the reader held.
    pub held: Checkpoint,
    /// The incompatible head it was offered.
    pub offered: SignedHead,
}

/// What a reader concluded about an offered head.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeadVerdict {
    /// Verified by ancestry and persisted. `from` is `None` for a stream's first head (trust on first
    /// use).
    Advanced {
        /// The previous checkpoint's `seq`, if any.
        from: Option<u64>,
        /// The new checkpoint's `seq`.
        to: u64,
    },
    /// The same head the reader already holds.
    AlreadyHeld,
    /// Lower than the checkpoint: a replay, ignored.
    StaleHead {
        /// What the reader holds.
        held: u64,
        /// What was offered.
        offered: u64,
    },
    /// Higher than the checkpoint, but the chain back to it could not be obtained. The checkpoint
    /// stays. Insufficient evidence — not acceptance, and not a fork.
    ContinuityUnavailable {
        /// What the reader holds.
        held: u64,
        /// What was offered.
        offered: u64,
    },
    /// Authenticated, and incompatible with the checkpoint: a different head at the same `seq`, or a
    /// chain that diverges before reaching it. Both are retained ([`HeadCheckpoints::forks`]).
    ForkedStream {
        /// What the reader holds.
        held: u64,
        /// What was offered.
        offered: u64,
    },
    /// The head does not point at a record of its own issuer.
    MismatchedRecord,
    /// No admissible path authenticates the head. It reads as absent.
    Unverifiable(UnverifiableReason),
    /// Authentic, but signed under a key its issuer has revoked. A head is a present-tense claim.
    SignedUnderRevokedKey,
    /// The advance was verified but could not be persisted, so it was **not** taken.
    CheckpointNotPersisted(String),
}

/// Where a reader's checkpoints live between restarts.
///
/// [`load`](Self::load) returns what was last persisted, or `None` for a reader that has never
/// persisted anything. An **error** means "state exists and cannot be read" — which
/// [`HeadCheckpoints::open`] treats as fatal rather than as empty.
pub trait CheckpointStore: Send + Sync {
    /// The last persisted state, if any.
    fn load(&self) -> Result<Option<Vec<u8>>, String>;
    /// Persist `state`, replacing what was there. Must not return `Ok` until the state would
    /// survive a restart of this store's backing medium.
    fn persist(&self, state: &[u8]) -> Result<(), String>;
}

/// An in-memory [`CheckpointStore`]. **Not durable**: it survives only as long as the value does.
/// For tests, and for simulating a restart by opening a second [`HeadCheckpoints`] over a clone.
///
/// One lock (lock-order table row 41): a leaf, held only for a synchronous copy.
#[derive(Clone, Debug, Default)]
pub struct MemoryCheckpointStore {
    inner: Arc<Mutex<MemoryState>>,
}

#[derive(Debug, Default)]
struct MemoryState {
    bytes: Option<Vec<u8>>,
    fail_persist: bool,
}

impl MemoryCheckpointStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Make every later `persist` fail — to test that an unpersisted advance is not taken.
    pub fn fail_persists(&self, fail: bool) {
        self.inner.lock().expect("lock").fail_persist = fail;
    }

    /// Replace the stored bytes — to test that unreadable state is refused.
    pub fn overwrite(&self, bytes: Vec<u8>) {
        self.inner.lock().expect("lock").bytes = Some(bytes);
    }
}

impl CheckpointStore for MemoryCheckpointStore {
    fn load(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.inner.lock().map_err(|e| e.to_string())?.bytes.clone())
    }

    fn persist(&self, state: &[u8]) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|e| e.to_string())?;
        if inner.fail_persist {
            return Err("persist refused (test)".to_string());
        }
        inner.bytes = Some(state.to_vec());
        Ok(())
    }
}

/// The persisted form: versioned, one entry per stream.
#[derive(Serialize, Deserialize)]
struct PersistedState {
    version: u32,
    checkpoints: Vec<(IssuerId, String, Checkpoint)>,
}

const STATE_VERSION: u32 = 1;

/// Why [`HeadCheckpoints::open`] refused to start.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// The store could not be read.
    Load(String),
    /// State exists but is not a readable checkpoint set. Starting empty instead would silently
    /// reset rollback protection, so the reader does not start.
    Unreadable(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Load(e) => write!(f, "checkpoint store could not be read: {e}"),
            Self::Unreadable(e) => write!(f, "persisted checkpoints are unreadable: {e}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// A reader's per-stream checkpoints, verified by ancestry and persisted before they move.
pub struct HeadCheckpoints<S: CheckpointStore> {
    store: S,
    checkpoints: BTreeMap<(IssuerId, String), Checkpoint>,
    forks: Vec<ForkRecord>,
}

impl<S: CheckpointStore> HeadCheckpoints<S> {
    /// Open over `store`, loading whatever it last persisted. Refuses unreadable state.
    pub fn open(store: S) -> Result<Self, OpenError> {
        let checkpoints = match store.load().map_err(OpenError::Load)? {
            None => BTreeMap::new(),
            Some(bytes) => {
                let state: PersistedState = serde_json::from_slice(&bytes)
                    .map_err(|e| OpenError::Unreadable(e.to_string()))?;
                if state.version != STATE_VERSION {
                    return Err(OpenError::Unreadable(format!(
                        "unknown checkpoint state version {}",
                        state.version
                    )));
                }
                state.checkpoints.into_iter().map(|(i, s, c)| ((i, s), c)).collect()
            }
        };
        Ok(Self { store, checkpoints, forks: Vec::new() })
    }

    /// The checkpoint held for `(issuer, stream)`.
    pub fn checkpoint(&self, issuer: &IssuerId, stream: &str) -> Option<Checkpoint> {
        self.checkpoints.get(&(issuer.clone(), stream.to_string())).copied()
    }

    /// Every fork this reader has seen since it was opened. Never resolved here.
    pub fn forks(&self) -> &[ForkRecord] {
        &self.forks
    }

    /// **Offer a head**, with whatever intermediate heads the presenter or the reader's own fetch
    /// supplied. The intermediates are only a pool to build the chain from: each one used is
    /// authenticated in turn, and any that do not fit are ignored.
    pub fn offer(
        &mut self,
        offered: &SignedHead,
        intermediates: &[SignedHead],
        members: &impl MemberKeySource,
        external: &TrustedExternalIssuers,
    ) -> HeadVerdict {
        let head = &offered.head;
        if head.record.issuer != head.issuer {
            return HeadVerdict::MismatchedRecord;
        }
        if let Some(refusal) = authenticate(offered, members, external) {
            return refusal;
        }

        let key = (head.issuer.clone(), head.stream.clone());
        let offered_digest = head.digest();
        let Some(held) = self.checkpoints.get(&key).copied() else {
            // Trust on first use: nothing to check continuity against.
            return self.advance(key, None, Checkpoint { seq: head.seq, digest: offered_digest });
        };

        if head.seq < held.seq {
            return HeadVerdict::StaleHead { held: held.seq, offered: head.seq };
        }
        if head.seq == held.seq {
            if offered_digest == held.digest {
                return HeadVerdict::AlreadyHeld;
            }
            return self.fork(key, held, offered.clone());
        }

        // Higher: walk back through `prev` until the checkpoint is met, a divergence is proven, or a
        // link is missing.
        let mut current = head.clone();
        loop {
            match current.prev {
                Some(prev) if prev == held.digest => {
                    return self.advance(
                        key,
                        Some(held.seq),
                        Checkpoint { seq: head.seq, digest: offered_digest },
                    );
                }
                // A genesis head above the checkpoint: this history never passes through it.
                None => return self.fork(key, held, offered.clone()),
                Some(prev) => {
                    let parent = intermediates.iter().find(|c| {
                        c.head.issuer == head.issuer
                            && c.head.stream == head.stream
                            && c.head.digest() == prev
                            && authenticate(c, members, external).is_none()
                    });
                    let Some(parent) = parent else {
                        return HeadVerdict::ContinuityUnavailable {
                            held: held.seq,
                            offered: head.seq,
                        };
                    };
                    if parent.head.seq >= current.seq {
                        // Not a descending chain: it proves nothing about continuity.
                        return HeadVerdict::ContinuityUnavailable {
                            held: held.seq,
                            offered: head.seq,
                        };
                    }
                    if parent.head.seq <= held.seq {
                        // The chain reached the checkpoint's height without meeting it: it diverged.
                        return self.fork(key, held, offered.clone());
                    }
                    current = parent.head.clone();
                }
            }
        }
    }

    fn advance(
        &mut self,
        key: (IssuerId, String),
        from: Option<u64>,
        next: Checkpoint,
    ) -> HeadVerdict {
        let mut proposed = self.checkpoints.clone();
        proposed.insert(key, next);
        let state = PersistedState {
            version: STATE_VERSION,
            checkpoints: proposed.iter().map(|((i, s), c)| (i.clone(), s.clone(), *c)).collect(),
        };
        let bytes = match serde_json::to_vec(&state) {
            Ok(b) => b,
            Err(e) => return HeadVerdict::CheckpointNotPersisted(e.to_string()),
        };
        // Persist first: an advance that would not survive a restart is not taken.
        if let Err(e) = self.store.persist(&bytes) {
            return HeadVerdict::CheckpointNotPersisted(e);
        }
        self.checkpoints = proposed;
        HeadVerdict::Advanced { from, to: next.seq }
    }

    fn fork(&mut self, key: (IssuerId, String), held: Checkpoint, offered: SignedHead) -> HeadVerdict {
        let offered_seq = offered.head.seq;
        self.forks.push(ForkRecord { issuer: key.0, stream: key.1, held, offered });
        HeadVerdict::ForkedStream { held: held.seq, offered: offered_seq }
    }
}

/// `None` if the head is authentic and currently authorised; otherwise the refusal.
fn authenticate(
    signed: &SignedHead,
    members: &impl MemberKeySource,
    external: &TrustedExternalIssuers,
) -> Option<HeadVerdict> {
    match verify_signed_by(
        &signed.head.issuer,
        &signed.head.canonical_bytes(),
        &signed.signature,
        members,
        external,
    ) {
        Authenticity::Current { .. } => None,
        Authenticity::Revoked { .. } => Some(HeadVerdict::SignedUnderRevokedKey),
        Authenticity::Unverifiable(reason) => Some(HeadVerdict::Unverifiable(reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::issuer::MemberKeys;
    use crate::knowledge::RecordId;
    use crate::node_id::NodeId;
    use ed25519_dalek::SigningKey;
    use std::collections::HashMap;

    struct Fixture {
        sk: SigningKey,
        issuer: IssuerId,
        members: HashMap<NodeId, MemberKeys>,
        ext: TrustedExternalIssuers,
    }

    fn fixture() -> Fixture {
        let sk = SigningKey::from_bytes(&[3u8; 32]);
        let pk = sk.verifying_key().to_bytes();
        let n = NodeId::new("127.0.0.1", 7301).unwrap();
        let members = HashMap::from([(n.clone(), MemberKeys { retained: vec![pk], ..Default::default() })]);
        Fixture { sk, issuer: IssuerId::for_node(&n), members, ext: TrustedExternalIssuers::new() }
    }

    impl Fixture {
        /// A signed head at `seq` on branch `branch`, chained to `prev`.
        fn head(&self, seq: u64, branch: u8, prev: Option<&SignedHead>) -> SignedHead {
            let head = Head {
                issuer: self.issuer.clone(),
                stream: "s".to_string(),
                record: RecordId { issuer: self.issuer.clone(), digest: [branch.wrapping_add(seq as u8); 32] },
                seq,
                prev: prev.map(|p| p.head.digest()),
            };
            let signature = mycelium_core::tls::sign_bytes(&self.sk, &head.canonical_bytes()).to_vec();
            SignedHead { head, signature }
        }

        /// Heads 1..=n on one branch, chained.
        fn chain(&self, n: u64, branch: u8) -> Vec<SignedHead> {
            let mut out: Vec<SignedHead> = Vec::new();
            for seq in 1..=n {
                let prev = out.last().cloned();
                out.push(self.head(seq, branch, prev.as_ref()));
            }
            out
        }

        fn reader(&self) -> HeadCheckpoints<MemoryCheckpointStore> {
            HeadCheckpoints::open(MemoryCheckpointStore::new()).unwrap()
        }
    }

    /// Hold head 10 of branch A, then return the reader.
    fn holding_ten(f: &Fixture, store: MemoryCheckpointStore) -> (HeadCheckpoints<MemoryCheckpointStore>, Vec<SignedHead>) {
        let a = f.chain(12, 0);
        let mut r = HeadCheckpoints::open(store).unwrap();
        assert_eq!(r.offer(&a[9], &[], &f.members, &f.ext), HeadVerdict::Advanced { from: None, to: 10 });
        (r, a)
    }

    /// A verified extension advances: 10 → 12 with 11 supplied.
    #[test]
    fn a_verified_extension_advances() {
        let f = fixture();
        let (mut r, a) = holding_ten(&f, MemoryCheckpointStore::new());
        assert_eq!(
            r.offer(&a[11], &[a[10].clone()], &f.members, &f.ext),
            HeadVerdict::Advanced { from: Some(10), to: 12 }
        );
        assert_eq!(r.checkpoint(&f.issuer, "s").unwrap().seq, 12);
    }

    /// **A higher `seq` alone proves nothing.** 12 without 11: continuity unavailable, and the
    /// checkpoint stays at 10.
    #[test]
    fn a_higher_head_without_its_ancestry_does_not_replace_the_checkpoint() {
        let f = fixture();
        let (mut r, a) = holding_ten(&f, MemoryCheckpointStore::new());
        assert_eq!(
            r.offer(&a[11], &[], &f.members, &f.ext),
            HeadVerdict::ContinuityUnavailable { held: 10, offered: 12 }
        );
        assert_eq!(r.checkpoint(&f.issuer, "s").unwrap().seq, 10);
    }

    /// **The reviewer's case.** The reader holds 10 on branch A; it is offered 12 from branch B, which
    /// shares heads 1–9 and diverges at 10. A fork, both retained, checkpoint unchanged.
    #[test]
    fn a_higher_head_from_a_branch_that_diverged_below_the_checkpoint_is_a_fork() {
        let f = fixture();
        let (mut r, a) = holding_ten(&f, MemoryCheckpointStore::new());
        let b10 = f.head(10, 7, Some(&a[8]));
        let b11 = f.head(11, 7, Some(&b10));
        let b12 = f.head(12, 7, Some(&b11));
        assert_eq!(
            r.offer(&b12, &[b11, b10], &f.members, &f.ext),
            HeadVerdict::ForkedStream { held: 10, offered: 12 }
        );
        assert_eq!(r.checkpoint(&f.issuer, "s").unwrap().seq, 10);
        assert_eq!(r.forks().len(), 1);
        assert_eq!(r.forks()[0].offered.head.seq, 12);
    }

    /// Same `seq`, different head: a fork. Same head: already held. Lower: stale.
    #[test]
    fn same_seq_forks_duplicates_and_stale_heads() {
        let f = fixture();
        let (mut r, a) = holding_ten(&f, MemoryCheckpointStore::new());
        assert_eq!(r.offer(&a[9], &[], &f.members, &f.ext), HeadVerdict::AlreadyHeld);
        let other_ten = f.head(10, 5, Some(&a[8]));
        assert_eq!(r.offer(&other_ten, &[], &f.members, &f.ext), HeadVerdict::ForkedStream { held: 10, offered: 10 });
        assert_eq!(r.offer(&a[4], &[], &f.members, &f.ext), HeadVerdict::StaleHead { held: 10, offered: 5 });
    }

    /// **Durable across restart.** A second reader opened over the same store holds 10, and still
    /// refuses a rolled-back head.
    #[test]
    fn checkpoints_survive_a_restart_and_keep_refusing_rollback() {
        let f = fixture();
        let store = MemoryCheckpointStore::new();
        let (r, a) = holding_ten(&f, store.clone());
        drop(r);
        let mut restarted = HeadCheckpoints::open(store).unwrap();
        assert_eq!(restarted.checkpoint(&f.issuer, "s").unwrap().seq, 10);
        assert_eq!(restarted.offer(&a[6], &[], &f.members, &f.ext), HeadVerdict::StaleHead { held: 10, offered: 7 });
    }

    /// Unreadable persisted state is refused, never treated as empty.
    #[test]
    fn unreadable_persisted_state_refuses_to_open() {
        let store = MemoryCheckpointStore::new();
        store.overwrite(b"not a checkpoint set".to_vec());
        assert!(matches!(HeadCheckpoints::open(store), Err(OpenError::Unreadable(_))));
    }

    /// An advance that cannot be persisted is not taken.
    #[test]
    fn an_advance_that_cannot_be_persisted_is_not_taken() {
        let f = fixture();
        let store = MemoryCheckpointStore::new();
        let (mut r, a) = holding_ten(&f, store.clone());
        store.fail_persists(true);
        assert!(matches!(r.offer(&a[10], &[], &f.members, &f.ext), HeadVerdict::CheckpointNotPersisted(_)));
        assert_eq!(r.checkpoint(&f.issuer, "s").unwrap().seq, 10);
    }

    /// A forged head reads as absent; a forged link breaks the chain rather than extending it.
    #[test]
    fn forged_heads_and_forged_links_do_not_count() {
        let f = fixture();
        let (mut r, a) = holding_ten(&f, MemoryCheckpointStore::new());
        let mut forged = a[10].clone();
        forged.signature[0] ^= 0xff;
        assert_eq!(
            r.offer(&forged, &[], &f.members, &f.ext),
            HeadVerdict::Unverifiable(UnverifiableReason::BadSignature)
        );
        assert_eq!(
            r.offer(&a[11], &[forged], &f.members, &f.ext),
            HeadVerdict::ContinuityUnavailable { held: 10, offered: 12 }
        );
    }

    /// A head from a key the issuer has revoked is refused: a head is a present-tense claim.
    #[test]
    fn a_head_signed_under_a_revoked_key_is_refused() {
        let mut f = fixture();
        let pk = f.sk.verifying_key().to_bytes();
        for keys in f.members.values_mut() {
            keys.revoked.insert(pk);
        }
        let a = f.chain(1, 0);
        assert_eq!(f.reader().offer(&a[0], &[], &f.members, &f.ext), HeadVerdict::SignedUnderRevokedKey);
    }

    /// A head pointing at another issuer's record is refused before anything else.
    #[test]
    fn a_head_must_point_at_its_own_issuers_record() {
        let f = fixture();
        let mut h = f.head(1, 0, None);
        h.head.record.issuer = IssuerId::new("someone-else").unwrap();
        assert_eq!(f.reader().offer(&h, &[], &f.members, &f.ext), HeadVerdict::MismatchedRecord);
    }

    /// A genesis head above the checkpoint is a different history, not a newer one.
    #[test]
    fn a_new_genesis_above_the_checkpoint_is_a_fork() {
        let f = fixture();
        let (mut r, _a) = holding_ten(&f, MemoryCheckpointStore::new());
        let restarted_stream = f.head(20, 9, None);
        assert_eq!(
            r.offer(&restarted_stream, &[], &f.members, &f.ext),
            HeadVerdict::ForkedStream { held: 10, offered: 20 }
        );
    }
}
