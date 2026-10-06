//! Content-addressed payload tier — the storage half the LangGraph checkpointer consumes.
//!
//! Metadata gossips everywhere (KV); **payloads do not** — they live in a content-addressed
//! store and are fetched from whichever peer holds them. A blob's id *is* its SHA-256, so
//! every fetch (disk or mesh) is verified against the address and dedup across immutable
//! checkpoints falls out for free.
//!
//! v1 limit, stated honestly: one blob ≤ [`MAX_BLOB_BYTES`] (a single-frame RPC reply);
//! chunked transfer via `ServiceHandle::bulk_call` is the named follow-up. `FsBlobStore`
//! copies the artifact library's `FsLibrarySource` semantics (temp-write + rename,
//! verify-on-read) and swaps out for the extracted artifact-library crate when that ships —
//! it is re-implemented here only because `mycelium-wasm-host` carries wasmtime
//! unconditionally.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mycelium::{CapFilter, Capability, CapabilityReg, GossipAgent};
use sha2::{Digest, Sha256};
use tracing::warn;

/// Hard per-blob ceiling: a blob must fit one RPC reply frame (KV/signal frames are
/// size-gated at ~9.94 MiB; 8 MiB leaves envelope headroom).
pub const MAX_BLOB_BYTES: usize = 8 * 1024 * 1024;

/// RPC kind for peer blob fetch: 32-byte id in, blob bytes (or empty for miss) out.
pub const BLOB_FETCH_KIND: &str = "reason.blob.fetch";

/// Capability advertised by nodes running [`spawn_blob_server`].
const BLOB_CAP_NS: &str = "reason";
const BLOB_CAP_NAME: &str = "blob-cache";

// ── Identity ─────────────────────────────────────────────────────────────────

/// Content address of a blob — its SHA-256. Equality of ids is equality of bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlobId(pub [u8; 32]);

impl BlobId {
    /// The content address of `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// Lowercase 64-hex form (also the on-disk filename).
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Parse a 64-hex string; `None` on wrong length or non-hex.
    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
            let hi = (chunk[0] as char).to_digit(16)?;
            let lo = (chunk[1] as char).to_digit(16)?;
            out[i] = ((hi << 4) | lo) as u8;
        }
        Some(Self(out))
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

// ── Local store ──────────────────────────────────────────────────────────────

/// What one node's disk holds for a blob id.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocalRead {
    /// No file for it.
    Absent,
    /// A copy that matches its content address.
    Valid(Bytes),
    /// A copy that no longer matches its address — damage at rest (or a partial legacy write).
    Damaged(Bytes),
}

/// Filesystem content-addressed blob store. One file per blob, named by its hex id.
///
/// No locks: writes are **complete-or-absent** (uniquely-named temp file + rename — the
/// `FsLibrarySource` discipline), so a concurrent reader never observes a partial blob,
/// and reads verify the hash: [`get`](FsBlobStore::get) never returns bad data, and [`read`](FsBlobStore::read)
/// says when a copy on disk is damaged rather than absent.
pub struct FsBlobStore {
    dir: PathBuf,
}

impl FsBlobStore {
    /// Open a blob directory, creating it (and parents) if absent.
    pub fn open(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn path_of(&self, id: &BlobId) -> PathBuf {
        self.dir.join(id.to_hex())
    }

    /// Store `bytes`, returning their content address. Idempotent — storing bytes the
    /// store already holds is a no-op returning the same id. Rejects blobs over
    /// [`MAX_BLOB_BYTES`] with `InvalidInput` (they could never travel the mesh).
    pub fn put(&self, bytes: &[u8]) -> std::io::Result<BlobId> {
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("blob is {} bytes; the single-frame ceiling is {MAX_BLOB_BYTES}", bytes.len()),
            ));
        }
        let id = BlobId::of(bytes);
        let path = self.path_of(&id);
        // A valid copy is a no-op. A damaged one is replaced: returning early because a file existed left
        // damage unrepairable — the mesh write-back and a client's re-upload both did nothing (S5).
        if matches!(self.read(&id), LocalRead::Valid(_)) {
            return Ok(id);
        }
        static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let tmp = self.dir.join(format!(
            ".tmp-{}-{}-{}",
            id.to_hex(),
            std::process::id(),
            TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(id)
    }

    /// Read the blob for `id`, verifying the content address. A hash mismatch (disk
    /// corruption, partial legacy write) is a miss — `None` + a warning, never bad bytes.
    /// [`read`](Self::read) says which.
    pub fn get(&self, id: &BlobId) -> Option<Bytes> {
        match self.read(id) {
            LocalRead::Valid(bytes) => Some(bytes),
            LocalRead::Absent | LocalRead::Damaged(_) => None,
        }
    }

    /// Read the blob for `id`, saying whether a copy is absent, valid, or **damaged** — present on disk
    /// but no longer matching its content address. Damage is not absence: it is evidence a reader must be
    /// able to see (realignment repairs S5).
    pub fn read(&self, id: &BlobId) -> LocalRead {
        let Ok(bytes) = std::fs::read(self.path_of(id)) else {
            return LocalRead::Absent;
        };
        if BlobId::of(&bytes) != *id {
            warn!(id = %id, "blob failed content verification on read — damaged at rest");
            return LocalRead::Damaged(Bytes::from(bytes));
        }
        LocalRead::Valid(Bytes::from(bytes))
    }

    /// Whether a file for `id` exists (unverified presence check).
    pub fn contains(&self, id: &BlobId) -> bool {
        self.path_of(id).exists()
    }
}

// ── Mesh serving ─────────────────────────────────────────────────────────────

/// RAII handle for a running blob server. Dropping it retracts the `reason/blob-cache`
/// capability and aborts the fetch loop.
pub struct BlobServerHandle {
    _cap: CapabilityReg,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for BlobServerHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// What the stock blob server answers for a copy that is damaged on its disk: non-empty, and the content
/// address of nothing a blob id could name, so a requester reads it as corrupt evidence (S5).
pub const DAMAGED_REPLY: &[u8] = b"mycelium-reason: the holder's copy of this blob is damaged at rest";

/// Advertise this node as a blob provider and serve [`BLOB_FETCH_KIND`] RPCs against
/// `store`. Reply is the blob bytes, [`DAMAGED_REPLY`] for a copy damaged on disk, or **empty bytes**
/// for a miss / malformed id —
/// unambiguous because the one blob whose bytes ARE empty (id = SHA-256 of `""`) is
/// never fetched over the mesh: [`MeshBlobStore::get`] answers it from the content
/// address alone (the LangGraph checkpointer stores `None` channel values as exactly
/// this blob, so the degenerate case is a real path, not a curiosity).
pub fn spawn_blob_server(agent: &Arc<GossipAgent>, store: Arc<FsBlobStore>) -> BlobServerHandle {
    let cap = agent
        .capabilities()
        .advertise_capability(Capability::new(BLOB_CAP_NS, BLOB_CAP_NAME), Duration::from_secs(30));
    let service = agent.service();
    let mut rx = service.rpc_rx(BLOB_FETCH_KIND);
    let task = tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let payload = req.payload();
            // A damaged copy is answered with `DAMAGED_REPLY`, which no content address matches, so the
            // requester counts it as corrupt rather than as a miss — whatever the damage left behind (a file
            // truncated to nothing would otherwise be the empty "miss"; one grown past the frame cap would
            // never arrive). An absent copy is the empty reply, as before (S5).
            let reply = match <[u8; 32]>::try_from(payload.as_ref()) {
                Ok(id) => match store.read(&BlobId(id)) {
                    LocalRead::Valid(bytes) => bytes,
                    LocalRead::Damaged(_) => Bytes::from_static(DAMAGED_REPLY),
                    LocalRead::Absent => Bytes::new(),
                },
                Err(_) => Bytes::new(),
            };
            service.rpc_respond(&req, reply);
        }
    });
    BlobServerHandle { _cap: cap, task }
}

// ── Mesh fetching ────────────────────────────────────────────────────────────

/// Why [`MeshBlobStore::fetch`] did not return a blob.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlobMiss {
    /// No provider holds it — every one answered *miss*, or there is none. Transient while blobs propagate.
    NotFound,
    /// At least one provider could not be reached (timeout or transport error) and none served it. Transient.
    Unavailable,
    /// Every copy anyone could serve — local or a provider's reply — fails the content address, and no
    /// provider was unreachable or merely lacked it. Not transient: damage or forgery, which waiting does
    /// not undo.
    Corrupt,
}

impl BlobMiss {
    fn classify(corrupt: usize, unreachable: usize, missed: usize) -> Self {
        if corrupt > 0 && unreachable == 0 && missed == 0 {
            Self::Corrupt
        } else if unreachable > 0 {
            Self::Unavailable
        } else {
            Self::NotFound
        }
    }

    /// The wire name the gateway route answers with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Unavailable => "unavailable",
            Self::Corrupt => "corrupt",
        }
    }
}

/// Local-first, mesh-fallback blob store: `get` serves a local hit, else asks each
/// `reason/blob-cache` provider in turn, verifies the reply against the content
/// address, and write-back caches it locally.
#[derive(Clone)]
pub struct MeshBlobStore {
    agent: Arc<GossipAgent>,
    local: Arc<FsBlobStore>,
    fetch_timeout: Duration,
}

impl MeshBlobStore {
    pub fn new(agent: Arc<GossipAgent>, local: Arc<FsBlobStore>, fetch_timeout: Duration) -> Self {
        Self { agent, local, fetch_timeout }
    }

    /// The local tier (write-back cache target).
    pub fn local(&self) -> &Arc<FsBlobStore> {
        &self.local
    }

    /// Store locally. Peers fetch it from here once this node runs [`spawn_blob_server`].
    pub fn put(&self, bytes: &[u8]) -> std::io::Result<BlobId> {
        self.local.put(bytes)
    }

    /// Local hit, else fetch from mesh providers. Every mesh reply is verified against
    /// the content address before being cached or returned — providers are untrusted.
    ///
    /// The **empty blob** (id = SHA-256 of `""`) is answered from the address alone:
    /// its bytes are fully determined by its id, and it *cannot* travel the fetch RPC,
    /// whose empty reply means "miss". Serializers do mint it (a typed `None` payload
    /// serializes to zero bytes), so this is a load-bearing case.
    pub async fn get(&self, id: &BlobId) -> Option<Bytes> {
        self.fetch(id).await.ok()
    }

    /// [`get`](Self::get), saying **why** a blob is not returned (realignment repairs S5: *"absence,
    /// temporary unavailability, authorization refusal and corrupt content stay distinguishable"*).
    ///
    /// **Corrupt** only when every copy currently on offer is bad — this node's and every **advertised**
    /// provider's (a holder offline past its capability lease is not counted): some copy (local, or a provider's
    /// reply) failed the content address, no provider was unreachable, and none merely lacked it. One bad
    /// provider beside an honest one that has not received the blob yet is **not found** — retriable — so
    /// a single faulty or hostile node cannot turn a blob that is still spreading into a permanent failure.
    /// Otherwise **unavailable** if a provider could not be reached, else **not found**.
    pub async fn fetch(&self, id: &BlobId) -> Result<Bytes, BlobMiss> {
        if *id == BlobId::of(&[]) {
            return Ok(Bytes::new());
        }
        // A damaged local copy counts as corrupt evidence, like a provider serving bad bytes.
        let mut corrupt = match self.local.read(id) {
            LocalRead::Valid(bytes) => return Ok(bytes),
            LocalRead::Damaged(_) => 1usize,
            LocalRead::Absent => 0,
        };
        let providers = self.agent.capabilities().resolve(&CapFilter::new(BLOB_CAP_NS, BLOB_CAP_NAME));
        let me = self.agent.node_id().clone();
        let (mut unreachable, mut missed) = (0usize, 0usize);
        for (node, _) in providers {
            if node == me {
                continue; // self is the local tier, already missed
            }
            let reply = self
                .agent
                .service()
                .rpc_call(node.clone(), BLOB_FETCH_KIND, Bytes::copy_from_slice(&id.0), self.fetch_timeout)
                .await;
            match reply {
                Ok(bytes) if !bytes.is_empty() && BlobId::of(&bytes) == *id => {
                    if let Err(e) = self.local.put(&bytes) {
                        warn!(id = %id, error = %e, "write-back cache of mesh blob failed");
                    }
                    return Ok(bytes);
                }
                Ok(bytes) if !bytes.is_empty() => {
                    corrupt += 1;
                    warn!(id = %id, provider = %node, "mesh blob failed content verification — trying next provider");
                }
                Ok(_) => missed += 1,       // the provider answered: it does not hold it (yet)
                Err(_) => unreachable += 1, // timeout or transport error: it may
            }
        }
        Err(BlobMiss::classify(corrupt, unreachable, missed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, FsBlobStore) {
        let dir = tempfile::tempdir().unwrap();
        let s = FsBlobStore::open(dir.path()).unwrap();
        (dir, s)
    }

    #[test]
    fn roundtrip_and_idempotency() {
        let (_d, s) = store();
        let id = s.put(b"payload").unwrap();
        assert_eq!(id, BlobId::of(b"payload"));
        assert_eq!(s.get(&id).unwrap().as_ref(), b"payload");
        assert!(s.contains(&id));
        // Idempotent: same bytes, same id, still one file.
        assert_eq!(s.put(b"payload").unwrap(), id);
    }

    #[test]
    fn hex_roundtrip() {
        let id = BlobId::of(b"x");
        assert_eq!(BlobId::from_hex(&id.to_hex()), Some(id));
        assert_eq!(BlobId::from_hex("zz"), None);
        assert_eq!(BlobId::from_hex(&"g".repeat(64)), None);
        assert_eq!(format!("{id}"), id.to_hex());
    }

    #[test]
    fn oversize_rejected() {
        let (_d, s) = store();
        let big = vec![0u8; MAX_BLOB_BYTES + 1];
        let err = s.put(&big).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        // The exact ceiling is accepted.
        assert!(s.put(&vec![0u8; MAX_BLOB_BYTES]).is_ok());
    }

    /// S5: why a blob is missing. Corrupt only when no copy currently on offer could still serve is good — not when one
    /// bad provider sits beside one that simply has not received it (the review of #542, finding 1).
    #[test]
    fn a_miss_is_classified_by_its_worst_evidence() {
        assert_eq!(BlobMiss::classify(0, 0, 0), BlobMiss::NotFound, "no copy and no provider");
        assert_eq!(BlobMiss::classify(0, 0, 2), BlobMiss::NotFound, "every provider answered miss");
        assert_eq!(BlobMiss::classify(0, 2, 0), BlobMiss::Unavailable, "a provider that could not be reached may hold it");
        assert_eq!(BlobMiss::classify(1, 0, 0), BlobMiss::Corrupt, "every copy anyone could serve is bad");
        assert_eq!(BlobMiss::classify(1, 0, 1), BlobMiss::NotFound, "an honest provider may still receive it");
        assert_eq!(BlobMiss::classify(1, 3, 0), BlobMiss::Unavailable, "an unreachable provider may hold a good copy");
        assert_eq!(
            [BlobMiss::NotFound, BlobMiss::Unavailable, BlobMiss::Corrupt].map(BlobMiss::as_str),
            ["not_found", "unavailable", "corrupt"]
        );
    }

    /// The second review of #542, finding 2: damage must be repairable. `put` returned early when a file
    /// existed, so writing the correct bytes over a damaged copy — the mesh write-back, or a client's
    /// re-upload — left the damage in place.
    #[test]
    fn putting_the_right_bytes_repairs_a_damaged_copy() {
        let (_d, s) = store();
        let id = s.put(b"honest bytes").unwrap();
        std::fs::write(s.path_of(&id), b"rot").unwrap();
        assert!(matches!(s.read(&id), LocalRead::Damaged(_)));
        assert_eq!(s.put(b"honest bytes").unwrap(), id);
        assert_eq!(s.read(&id), LocalRead::Valid(Bytes::from_static(b"honest bytes")));
    }

    #[test]
    fn corruption_returns_none() {
        let (_d, s) = store();
        let id = s.put(b"honest bytes").unwrap();
        // Corrupt the file behind the store's back.
        std::fs::write(s.path_of(&id), b"tampered").unwrap();
        assert!(s.get(&id).is_none(), "verify-on-read catches the tamper");
        assert!(s.contains(&id), "contains() is presence-only, unverified");
    }
}
