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
use tracing::{debug, warn};

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
/// `FsLibrarySource` discipline), so a concurrent reader never observes a partial blob — though not
/// crash-durable (no fsync; a crash can leave a short file, which reads as damaged and is repaired by
/// the next put of the right bytes),
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

    /// Store `bytes`, returning their content address. Idempotent — storing bytes the store already
    /// holds is a no-op returning the same id, at the cost of reading the existing copy to compare (a
    /// copy that differs is damaged and is replaced). Rejects blobs over
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
        // A copy holding exactly these bytes is a no-op. Anything else at the path — damaged, truncated,
        // unreadable — is replaced: returning early because a file existed left damage unrepairable (S5).
        // The check compares the length first and the bytes only on a match — no second hash.
        if std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64)
            && std::fs::read(&path).is_ok_and(|existing| existing == bytes)
        {
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
        let bytes = match std::fs::read(self.path_of(id)) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LocalRead::Absent,
            // EIO, EACCES, a directory where the file should be: the copy is there and cannot be read —
            // damage at rest, not absence (an `Absent` would be the retriable miss, forever).
            Err(e) => {
                warn!(id = %id, error = %e, "blob present but unreadable — damaged at rest");
                return LocalRead::Damaged(Bytes::new());
            }
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

/// What one provider's reply to a blob fetch says.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ReplyOutcome {
    /// The blob: its bytes match the content address.
    Valid,
    /// Empty: the provider does not hold it.
    Miss,
    /// Bytes that fail the content address — a damaged copy ([`DAMAGED_REPLY`]) or a forgery.
    Corrupt,
    /// The provider's RPC layer answered for it — a caller-context or provider-enforcement refusal, a JSON object
    /// with an `error` — so its store was never asked (#564). `transient` unless its `reason` is one that holds until
    /// something changes ([`PERMANENT_REFUSALS`]); `reason` as given, or `unspecified`.
    Refused { transient: bool, reason: String },
}

/// Refusal reasons that waiting does not undo: an envelope that is malformed, mismatched, unsigned or badly signed; a
/// removed member; a denied action or authority not established; a malformed call. Any other reason — an unknown signer
/// or a marker not yet gossiped, capacity, a timeout, a reason this version does not know, or none (an older peer) — is
/// treated as transient: retried within the caller's deadline rather than ending the wait (#564's review).
const PERMANENT_REFUSALS: &[&str] = &[
    "malformed", "via_mismatch", "unsigned", "bad_signature", "removed",
    "action_denied", "authority_not_established", "malformed_call", "caller_context_too_large",
];

/// Classify one reply. The content address is checked first, so a blob that happens to be such JSON is still a blob.
fn reply_outcome(bytes: &[u8], id: &BlobId) -> ReplyOutcome {
    if bytes.is_empty() {
        return ReplyOutcome::Miss;
    }
    if BlobId::of(bytes) == *id {
        return ReplyOutcome::Valid;
    }
    let Ok(serde_json::Value::Object(o)) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return ReplyOutcome::Corrupt;
    };
    if !o.get("error").is_some_and(|e| e.is_string()) {
        return ReplyOutcome::Corrupt;
    }
    let reason = o.get("reason").and_then(|r| r.as_str()).unwrap_or("unspecified").to_string();
    ReplyOutcome::Refused { transient: !PERMANENT_REFUSALS.contains(&reason.as_str()), reason }
}

/// Why [`MeshBlobStore::fetch`] did not return a blob.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlobMiss {
    /// No provider holds it — every one answered *miss*, or there is none. Transient while blobs propagate.
    NotFound,
    /// At least one provider could not be reached (timeout or transport error) or refused for now (its RPC layer
    /// answered for it with a transient reason, #564), and none served it. Transient.
    Unavailable,
    /// Every copy anyone could serve — local or a provider's reply — fails the content address, and no
    /// provider was unreachable or merely lacked it. Not transient: damage or forgery, which waiting does
    /// not undo.
    Corrupt,
    /// Every holder that could be asked refused, for a reason waiting does not undo (a removed member, a denied
    /// action, a bad envelope — [`PERMANENT_REFUSALS`]), and none could not be reached or merely lacked it. Not
    /// transient; the route answers 403 `refused` (#564's review: *authorization refusal stays distinguishable*).
    Refused,
}

impl BlobMiss {
    fn classify(corrupt: usize, unreachable: usize, missed: usize, refused: usize) -> Self {
        if unreachable > 0 {
            Self::Unavailable
        } else if missed > 0 {
            Self::NotFound
        } else if refused > 0 {
            Self::Refused // a refusing holder may hold a good copy, so a refusal outranks corrupt evidence
        } else if corrupt > 0 {
            Self::Corrupt
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
            Self::Refused => "refused",
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
        let local_damaged = corrupt > 0;
        let providers = self.agent.capabilities().resolve(&CapFilter::new(BLOB_CAP_NS, BLOB_CAP_NAME));
        let me = self.agent.node_id().clone();
        let (mut unreachable, mut missed, mut refused) = (0usize, 0usize, 0usize);
        let mut asked: Vec<String> = Vec::new(); // who answered what, for the miss log (#563)
        for (node, _) in providers {
            if node == me {
                continue; // self is the local tier, already missed
            }
            let reply = self
                .agent
                .service()
                .rpc_call(node.clone(), BLOB_FETCH_KIND, Bytes::copy_from_slice(&id.0), self.fetch_timeout)
                .await;
            match reply.as_ref().map(|bytes| reply_outcome(bytes, id)) {
                Ok(ReplyOutcome::Valid) => {
                    let bytes = reply.expect("a valid outcome is an Ok reply");
                    if let Err(e) = self.local.put(&bytes) {
                        warn!(id = %id, error = %e, "write-back cache of mesh blob failed");
                    }
                    return Ok(bytes);
                }
                Ok(ReplyOutcome::Corrupt) => {
                    corrupt += 1;
                    asked.push(format!("{node}=corrupt"));
                    warn!(id = %id, provider = %node, "mesh blob failed content verification — trying next provider");
                }
                Ok(ReplyOutcome::Refused { transient, reason }) => {
                    // The holder's RPC layer answered for it, so its store was never asked (#564). A refusal that
                    // passes as gossip converges (an unknown signer, a marker not yet seen, or no reason — an older
                    // peer) is a holder that could not be asked yet; any other holds until something changes.
                    if transient { unreachable += 1 } else { refused += 1 }
                    asked.push(format!("{node}=refused({reason})"));
                }
                Ok(ReplyOutcome::Miss) => { missed += 1; asked.push(format!("{node}=miss")); } // it does not hold it (yet)
                Err(e) => { unreachable += 1; asked.push(format!("{node}=unreachable({e})")); } // it may
            }
        }
        let miss = BlobMiss::classify(corrupt, unreachable, missed, refused);
        // Which not-found case this was — no other provider resolved, or each answered miss — is what #563 could not
        // tell from a CI failure. Debug: a retriable miss is a state clients poll through, one line per poll per blob.
        debug!(id = %id, reason = miss.as_str(), local_damaged,
               providers = if asked.is_empty() { "none resolved besides this node".to_string() } else { asked.join(", ") },
               "blob fetch missed");
        Err(miss)
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

    /// #564: a refusal from the provider's RPC layer — caller context, provider enforcement — is not a bad copy; it is
    /// a holder that could not be asked. A damaged copy and forged bytes stay corrupt; a JSON blob is still a blob.
    #[test]
    fn an_rpc_refusal_is_not_a_corrupt_copy() {
        let id = BlobId::of(b"payload");
        assert_eq!(reply_outcome(b"payload", &id), ReplyOutcome::Valid);
        assert_eq!(reply_outcome(b"", &id), ReplyOutcome::Miss);
        assert_eq!(reply_outcome(DAMAGED_REPLY, &id), ReplyOutcome::Corrupt);
        assert_eq!(reply_outcome(b"forged", &id), ReplyOutcome::Corrupt);
        let refused = |transient: bool, reason: &str| ReplyOutcome::Refused { transient, reason: reason.into() };
        assert_eq!(reply_outcome(br#"{"error":"caller context refused: unknown signer","reason":"unknown_signer"}"#, &id), refused(true, "unknown_signer"));
        assert_eq!(reply_outcome(br#"{"error":"caller context refused: removed","reason":"removed"}"#, &id), refused(false, "removed"));
        assert_eq!(reply_outcome(br#"{"error":"denied","reason":"action_denied","data":null}"#, &id), refused(false, "action_denied"));
        assert_eq!(reply_outcome(br#"{"error":"full","reason":"at_capacity","data":null}"#, &id), refused(true, "at_capacity"));
        assert_eq!(reply_outcome(br#"{"error":"caller context refused: unknown signer"}"#, &id), refused(true, "unspecified"),
                   "an older peer's refusal names no reason: retriable");
        assert_eq!(reply_outcome(br#"{"error":7}"#, &id), ReplyOutcome::Corrupt, "not a refusal's shape");
        let json_blob = br#"{"error":"this is the payload"}"#;
        assert_eq!(reply_outcome(json_blob, &BlobId::of(json_blob)), ReplyOutcome::Valid);
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
        assert_eq!(BlobMiss::classify(0, 0, 0, 0), BlobMiss::NotFound, "no copy and no provider");
        assert_eq!(BlobMiss::classify(0, 0, 2, 0), BlobMiss::NotFound, "every provider answered miss");
        assert_eq!(BlobMiss::classify(0, 2, 0, 0), BlobMiss::Unavailable, "a provider that could not be reached may hold it");
        assert_eq!(BlobMiss::classify(1, 0, 0, 0), BlobMiss::Corrupt, "every copy anyone could serve is bad");
        assert_eq!(BlobMiss::classify(1, 0, 1, 0), BlobMiss::NotFound, "an honest provider may still receive it");
        assert_eq!(BlobMiss::classify(1, 3, 0, 0), BlobMiss::Unavailable, "an unreachable provider may hold a good copy");
        // #564: a permanent refusal from every holder that could be asked is its own answer; a transient one counts
        // with the unreachable; a miss or an unreachable holder beside a refusal keeps the wait going.
        assert_eq!(BlobMiss::classify(0, 0, 0, 2), BlobMiss::Refused, "every holder refused, for good");
        assert_eq!(BlobMiss::classify(1, 0, 0, 1), BlobMiss::Refused, "a refusing holder may hold a good copy");
        assert_eq!(BlobMiss::classify(0, 0, 1, 1), BlobMiss::NotFound, "one lacks it, one refused: it may yet arrive");
        assert_eq!(BlobMiss::classify(0, 1, 0, 1), BlobMiss::Unavailable, "one unreachable (or refused for now)");
        assert_eq!(
            [BlobMiss::NotFound, BlobMiss::Unavailable, BlobMiss::Corrupt, BlobMiss::Refused].map(BlobMiss::as_str),
            ["not_found", "unavailable", "corrupt", "refused"]
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

    /// The third review of #542: a copy the disk cannot read (EIO, EACCES — here a directory where the
    /// file should be) is damage at rest, not absence. It read as `Absent`, the retriable miss.
    #[test]
    fn an_unreadable_copy_is_damaged_not_absent() {
        let (_d, s) = store();
        let id = BlobId::of(b"unreadable");
        std::fs::create_dir(s.path_of(&id)).unwrap();
        assert!(matches!(s.read(&id), LocalRead::Damaged(_)));
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
