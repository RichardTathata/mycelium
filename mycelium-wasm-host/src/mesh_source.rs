//! Pulling artifacts **over the cluster mesh** — closes §E.4.4 (content-addressed distribution)
//! on Mycelium's public API.
//!
//! A node that holds artifacts runs [`serve_artifacts`], which answers `artifact.fetch` RPCs with
//! the bytes for a requested [`ArtifactId`]. Any node can then [`pull_artifact`] from a peer; the
//! bytes are verified against the content address on arrival, so the *source is untrusted* (a peer
//! returning the wrong bytes is rejected, exactly like any other [`ArtifactSource`]).
//!
//! Transport is RPC on the gossip frame. A **whole-object** pull (`artifact.fetch`) fits one frame
//! (≤ `MAX_FRAME_BYTES` = 10 MiB) — right for a component. A blob past that crosses in **ranges**
//! (zero-gaps Z3, D3): `artifact.size`, then `artifact.fetch_range` pieces of at most
//! [`MESH_RANGE_CHUNK_BYTES`], each a fraction of a frame, staged to disk and hashed as they land
//! through [`DiskStagedSource`](crate::DiskStagedSource) over a [`MeshRangedFetcher`] — the same
//! stage an object store fills, so a stem's mesh path keeps no blob in memory.
//!
//! [`ArtifactSource`]: crate::ArtifactSource

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use mycelium::{CapFilter, GossipAgent, NodeId};

use crate::artifact::{verify_artifact, ArtifactId, ArtifactSource};

/// RPC kind used to request artifact bytes by content address.
pub const ARTIFACT_FETCH_KIND: &str = "artifact.fetch";
/// RPC kind: the size of an artifact (payload = 32-byte id; reply = 8-byte big-endian length, or
/// empty when not held).
pub const ARTIFACT_SIZE_KIND: &str = "artifact.size";
/// RPC kind: a range of an artifact (payload = 32-byte id · u64 BE offset · u64 BE len; reply = at
/// most [`MESH_RANGE_CHUNK_BYTES`] bytes from the offset, or empty when not held or out of range).
pub const ARTIFACT_RANGE_KIND: &str = "artifact.fetch_range";
/// The most bytes one range reply carries: 4 MiB, the blob runtime's own chunk and a fraction of
/// the frame, so a range never meets the cap a whole object does.
pub const MESH_RANGE_CHUNK_BYTES: u64 = 4 * 1024 * 1024;

fn range_request(id: &ArtifactId, offset: u64, len: u64) -> Vec<u8> {
    let mut p = Vec::with_capacity(48);
    p.extend_from_slice(id.as_bytes());
    p.extend_from_slice(&offset.to_be_bytes());
    p.extend_from_slice(&len.to_be_bytes());
    p
}

fn parse_range_request(payload: &[u8]) -> Option<(ArtifactId, u64, u64)> {
    if payload.len() != 48 { return None; }
    let id = ArtifactId::from_bytes(<[u8; 32]>::try_from(&payload[..32]).ok()?);
    let offset = u64::from_be_bytes(payload[32..40].try_into().ok()?);
    let len = u64::from_be_bytes(payload[40..48].try_into().ok()?);
    Some((id, offset, len))
}

fn parse_id(payload: &[u8]) -> Option<ArtifactId> {
    <[u8; 32]>::try_from(payload).ok().map(ArtifactId::from_bytes)
}

/// The size of `id` as `source` holds it: the ranged view when there is one, else the whole object.
fn held_size(source: &(dyn ArtifactSource + Send + Sync), id: &ArtifactId) -> Option<u64> {
    match source.as_ranged() {
        Some(r) => r.size(id),
        None => source.fetch(id).map(|b| b.len() as u64),
    }
}

/// `len` bytes of `id` at `offset`, capped at [`MESH_RANGE_CHUNK_BYTES`]: the ranged view when there
/// is one, else a slice of the whole object (an in-memory source serves ranges too).
fn held_range(source: &(dyn ArtifactSource + Send + Sync), id: &ArtifactId, offset: u64, len: u64) -> Option<Bytes> {
    let len = len.min(MESH_RANGE_CHUNK_BYTES);
    match source.as_ranged() {
        Some(r) => r.fetch_range(id, offset, len),
        None => {
            let whole = source.fetch(id)?;
            let start = usize::try_from(offset).ok()?;
            if start > whole.len() { return None; }
            let end = start.saturating_add(usize::try_from(len).ok()?).min(whole.len());
            Some(whole.slice(start..end))
        }
    }
}

/// Serve artifacts held by `source` to the cluster: one task answering three RPC kinds —
/// `artifact.fetch` (whole object, payload = 32-byte [`ArtifactId`], empty if not held),
/// `artifact.size` and `artifact.fetch_range` (zero-gaps Z3: a blob past the frame cap crosses in
/// ranges; a ranged `source` serves them from disk, any other from the whole object). Returns the
/// serve task handle (drop/abort to stop serving; it also ends on shutdown).
pub fn serve_artifacts(
    agent: Arc<GossipAgent>,
    source: Arc<dyn ArtifactSource + Send + Sync>,
) -> tokio::task::JoinHandle<()> {
    let mut fetch_rx = agent.service().rpc_rx(ARTIFACT_FETCH_KIND);
    let mut size_rx = agent.service().rpc_rx(ARTIFACT_SIZE_KIND);
    let mut range_rx = agent.service().rpc_rx(ARTIFACT_RANGE_KIND);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                req = fetch_rx.recv() => {
                    let Some(req) = req else { break };
                    let bytes = parse_id(req.payload().as_ref()).and_then(|id| source.fetch(&id)).unwrap_or_default();
                    agent.service().rpc_respond(&req, bytes);
                }
                req = size_rx.recv() => {
                    let Some(req) = req else { break };
                    let reply = parse_id(req.payload().as_ref())
                        .and_then(|id| held_size(source.as_ref(), &id))
                        .map(|n| Bytes::copy_from_slice(&n.to_be_bytes()))
                        .unwrap_or_default();
                    agent.service().rpc_respond(&req, reply);
                }
                req = range_rx.recv() => {
                    let Some(req) = req else { break };
                    let reply = parse_range_request(req.payload().as_ref())
                        .and_then(|(id, off, len)| held_range(source.as_ref(), &id, off, len))
                        .unwrap_or_default();
                    agent.service().rpc_respond(&req, reply);
                }
            }
        }
    })
}

/// A [`RangedBlobFetcher`](crate::http_source::RangedBlobFetcher) over mesh peers (zero-gaps Z3):
/// `size` asks `artifact.size` of each candidate holder until one answers and remembers it;
/// `fetch_range` asks that holder for `artifact.fetch_range` pieces. Hand it to
/// [`DiskStagedSource`](crate::DiskStagedSource) and a blob past the frame cap stages to disk in
/// pieces, hashed as it lands, exactly as it would from an object store. Holders come from a fixed
/// list, the capability ring (`librarian_filter()`), or both, as for [`MeshArtifactSource`].
pub struct MeshRangedFetcher {
    agent:           Arc<GossipAgent>,
    providers:       Vec<NodeId>,
    provider_filter: Option<CapFilter>,
    timeout:         Duration,
    holders:         Mutex<HashMap<ArtifactId, NodeId>>,
}

impl MeshRangedFetcher {
    /// Pull from a fixed provider list.
    pub fn new(agent: Arc<GossipAgent>, providers: Vec<NodeId>, timeout: Duration) -> Self {
        Self { agent, providers, provider_filter: None, timeout, holders: Mutex::new(HashMap::new()) }
    }

    /// Discover holders through the capability ring (e.g. `librarian_filter()`).
    pub fn resolving(agent: Arc<GossipAgent>, filter: CapFilter, timeout: Duration) -> Self {
        Self { agent, providers: Vec::new(), provider_filter: Some(filter), timeout, holders: Mutex::new(HashMap::new()) }
    }

    fn candidates(&self, id: &ArtifactId) -> Vec<NodeId> {
        let mut c: Vec<NodeId> = self.holders.lock().unwrap().get(id).cloned().into_iter().collect();
        for p in &self.providers {
            if !c.contains(p) { c.push(p.clone()); }
        }
        if let Some(filter) = &self.provider_filter {
            for (node, _cap) in self.agent.capabilities().resolve(filter) {
                if !c.contains(&node) { c.push(node); }
            }
        }
        c
    }

    async fn ask(&self, provider: &NodeId, kind: &str, payload: Vec<u8>) -> Option<Bytes> {
        self.agent.service().rpc_call(provider.clone(), kind, payload, self.timeout).await.ok().filter(|b| !b.is_empty())
    }
}

#[async_trait::async_trait]
impl crate::http_source::BlobFetcher for MeshRangedFetcher {
    /// Whole-object pull, verified — the small-artifact path; a blob past the frame cap answers
    /// `Ok(None)` here and is the ranged path's.
    async fn fetch_remote(&self, id: &ArtifactId) -> Result<Option<Bytes>, String> {
        for provider in self.candidates(id) {
            if let Some(bytes) = pull_artifact(&self.agent, provider.clone(), id, self.timeout).await {
                self.holders.lock().unwrap().insert(*id, provider);
                return Ok(Some(bytes));
            }
        }
        Ok(None)
    }
}

#[async_trait::async_trait]
impl crate::http_source::RangedBlobFetcher for MeshRangedFetcher {
    async fn size(&self, id: &ArtifactId) -> Result<Option<u64>, String> {
        for provider in self.candidates(id) {
            if let Some(reply) = self.ask(&provider, ARTIFACT_SIZE_KIND, id.as_bytes().to_vec()).await
                && let Ok(arr) = <[u8; 8]>::try_from(reply.as_ref())
            {
                self.holders.lock().unwrap().insert(*id, provider);
                return Ok(Some(u64::from_be_bytes(arr)));
            }
        }
        Ok(None)
    }

    async fn fetch_range(&self, id: &ArtifactId, offset: u64, len: u64) -> Result<Option<Bytes>, String> {
        if len == 0 {
            return Ok(Some(Bytes::new()));
        }
        let len = len.min(MESH_RANGE_CHUNK_BYTES);
        for provider in self.candidates(id) {
            if let Some(reply) = self.ask(&provider, ARTIFACT_RANGE_KIND, range_request(id, offset, len)).await {
                self.holders.lock().unwrap().insert(*id, provider);
                return Ok(Some(reply));
            }
        }
        Ok(None)
    }
}

/// Pull the bytes for `id` from `provider` over the mesh, verifying the content address on arrival.
/// `None` if the peer doesn't hold it, the call times out, or the returned bytes don't match `id`
/// (an untrusted peer cannot substitute bytes).
pub async fn pull_artifact(
    agent: &GossipAgent,
    provider: NodeId,
    id: &ArtifactId,
    timeout: Duration,
) -> Option<Bytes> {
    let reply = agent
        .service()
        .rpc_call(provider, ARTIFACT_FETCH_KIND, id.as_bytes().to_vec(), timeout)
        .await
        .ok()?;
    if reply.is_empty() || verify_artifact(&reply, id).is_err() {
        return None;
    }
    Some(reply)
}

/// An [`ArtifactSource`] backed by mesh peers. Because the trait's `fetch` is synchronous, bytes
/// must be [`prefetch`](Self::prefetch)ed (async, verified) into the cache before
/// `WasmHost::provision` reads them; `fetch` then serves from that cache. (Transparent on-demand
/// mesh pull would require an async `ArtifactSource` — a deliberate future refinement.)
///
/// Holders come from a fixed provider list ([`new`](Self::new)), the capability ring
/// ([`resolving`](Self::resolving) — e.g. `librarian_filter()`), or both: prefetch tries the
/// fixed list first, then live-resolved providers.
pub struct MeshArtifactSource {
    agent:           Arc<GossipAgent>,
    providers:       Vec<NodeId>,
    /// Holders discovered live at prefetch time by resolving this filter against the
    /// capability ring — the no-hardcoded-provider path (design §6).
    provider_filter: Option<CapFilter>,
    timeout:         Duration,
    cache:           Mutex<HashMap<ArtifactId, Bytes>>,
}

impl MeshArtifactSource {
    /// Pull from a fixed provider list (tests, fixed topologies).
    pub fn new(agent: Arc<GossipAgent>, providers: Vec<NodeId>, timeout: Duration) -> Self {
        Self { agent, providers, provider_filter: None, timeout, cache: Mutex::new(HashMap::new()) }
    }

    /// Discover holders through the capability ring instead of a fixed list: each `prefetch`
    /// resolves `filter` (e.g. `librarian_filter()`) and tries the matching nodes in order. A
    /// holder that appears, moves, or dies needs no reconfiguration here — its capability
    /// advertisement is the discovery.
    pub fn resolving(agent: Arc<GossipAgent>, filter: CapFilter, timeout: Duration) -> Self {
        Self {
            agent,
            providers: Vec::new(),
            provider_filter: Some(filter),
            timeout,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Pull `id` from the first provider that has it into the local cache (verified on arrival).
    /// Returns whether it is now cached. Idempotent — a cached id short-circuits. Misses are
    /// cheap (one RPC per tried holder), so a resolved holder set needs no per-artifact routing.
    pub async fn prefetch(&self, id: &ArtifactId) -> bool {
        if self.cache.lock().unwrap().contains_key(id) {
            return true;
        }
        let mut candidates = self.providers.clone();
        if let Some(filter) = &self.provider_filter {
            for (node, _cap) in self.agent.capabilities().resolve(filter) {
                if !candidates.contains(&node) {
                    candidates.push(node);
                }
            }
        }
        for provider in candidates {
            if let Some(bytes) = pull_artifact(&self.agent, provider, id, self.timeout).await {
                self.cache.lock().unwrap().insert(*id, bytes);
                return true;
            }
        }
        false
    }
}

impl MeshArtifactSource {
    /// How many verified artifacts the cache holds — what a stem re-serves to peers (X2).
    pub fn cached_len(&self) -> usize {
        self.cache.lock().unwrap().len()
    }
}

impl ArtifactSource for MeshArtifactSource {
    fn fetch(&self, id: &ArtifactId) -> Option<Bytes> {
        self.cache.lock().unwrap().get(id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::RangedArtifactSource;
    use crate::InMemorySource;

    fn alloc_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    async fn agent(port: u16, bootstrap: Option<u16>) -> Arc<GossipAgent> {
        let id = NodeId::new("127.0.0.1", port).unwrap();
        let cfg = mycelium::GossipConfig {
            bind_port: port,
            bootstrap_peers: bootstrap
                .map(|b| vec![NodeId::new("127.0.0.1", b).unwrap()])
                .unwrap_or_default(),
            ..Default::default()
        };
        let a = Arc::new(GossipAgent::new(id, cfg));
        a.start().await.expect("agent start");
        a
    }

    /// Zero-gaps Z3 (D3): a blob **past the frame cap** crosses the mesh in ranges — `artifact.size`
    /// then `artifact.fetch_range` pieces, each a fraction of a frame — and lands on disk verified,
    /// through the same `DiskStagedSource` an object store stages through. The whole-object
    /// `artifact.fetch` on the same id stays refused by the cap (its reply would be one frame).
    /// Written before the ranged kinds existed and seen failing to compile.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_blob_past_the_frame_cap_crosses_the_mesh_in_ranges() {
        let a_port = alloc_port();
        let a = agent(a_port, None).await;
        let b = agent(alloc_port(), Some(a_port)).await;
        for _ in 0..80 {
            if !a.peers().is_empty() && !b.peers().is_empty() { break; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        // 12 MiB of a deterministic pattern: past MAX_FRAME_BYTES (10 MiB), small enough to hash fast.
        let big: Vec<u8> = (0..12u32 * 1024 * 1024).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let lib_dir = std::env::temp_dir().join(format!("mycelium-ranged-mesh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&lib_dir);
        let lib = Arc::new(crate::FsLibrarySource::open(&lib_dir).unwrap());
        let id = lib.store(&big).unwrap();
        let _serve = serve_artifacts(Arc::clone(&a), lib as Arc<dyn ArtifactSource + Send + Sync>);

        // The whole-object path: refused by the frame cap, as before.
        let whole = MeshArtifactSource::new(Arc::clone(&b), vec![a.node_id().clone()], Duration::from_secs(3));
        assert!(!whole.prefetch(&id).await, "a 12 MiB reply cannot ride one frame");

        // The ranged path: staged to disk, verified, in pieces.
        let fetcher = Arc::new(MeshRangedFetcher::new(Arc::clone(&b), vec![a.node_id().clone()], Duration::from_secs(5)));
        let stage_dir = lib_dir.join("stage");
        let staged = crate::DiskStagedSource::open(fetcher, &stage_dir).unwrap();
        let mut ok = false;
        for _ in 0..10 {
            if staged.stage_artifact(&id).await { ok = true; break; }
        }
        assert!(ok, "the blob should stage over the mesh in ranges");
        assert_eq!(staged.size(&id), Some(big.len() as u64));
        assert_eq!(staged.fetch_range(&id, 0, 16).as_deref(), Some(&big[..16]));
        assert_eq!(crate::artifact::ArtifactId::of(&std::fs::read(stage_dir.join(id.to_hex())).unwrap()), id, "staged bytes hash to the id");

        a.shutdown().await;
        b.shutdown().await;
        let _ = std::fs::remove_dir_all(&lib_dir);
    }

    #[tokio::test]
    async fn pulls_and_verifies_an_artifact_from_a_peer() {
        // Server A holds the artifact and serves it; client B pulls it over the mesh.
        let a_port = alloc_port();
        let a = agent(a_port, None).await;
        let b = agent(alloc_port(), Some(a_port)).await;

        // wait until peered both ways
        for _ in 0..80 {
            if !a.peers().is_empty() && !b.peers().is_empty() { break; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        let mut src = InMemorySource::new();
        let id = src.insert(Bytes::from_static(b"artifact-over-the-mesh"));
        let _serve = serve_artifacts(Arc::clone(&a), Arc::new(src));

        // B pulls by content address, with retry for the RPC path to warm up.
        let mesh = MeshArtifactSource::new(Arc::clone(&b), vec![a.node_id().clone()], Duration::from_secs(2));
        let mut ok = false;
        for _ in 0..20 {
            if mesh.prefetch(&id).await { ok = true; break; }
        }
        assert!(ok, "B should pull the artifact from A over the mesh");
        assert_eq!(mesh.fetch(&id).as_deref(), Some(&b"artifact-over-the-mesh"[..]));

        // An id no peer holds → not cached.
        let unknown = ArtifactId::of(b"nobody has this");
        assert!(!mesh.prefetch(&unknown).await);
        assert!(mesh.fetch(&unknown).is_none());

        a.shutdown().await;
        b.shutdown().await;
    }
}
