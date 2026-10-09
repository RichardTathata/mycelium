//! The **object-store source** — pulling artifacts from an HTTP(S) blob store
//! (`docs/design/artifact-library.md` §2.2, sequencing step 6).
//!
//! Two pieces, deliberately separated:
//!
//! - [`BlobFetcher`] — the *async* remote-fetch face and the vendor extension point: implement
//!   it against any backend (an AWS SDK client with SigV4, an OCI registry, a Warg client) and
//!   hand it to [`PrefetchingSource`]. The shipped implementation is [`HttpLibrarySource`]:
//!   plain `GET {base_url}/{artifact-hex}`, which covers S3-compatible stores (public,
//!   bucket-policy, or static-header auth), nginx/CDN blob directories, and artifact servers.
//!   Native SigV4 request signing is out of scope by choice — that's a vendor SDK's job, and
//!   the trait is where such an SDK plugs in.
//! - [`PrefetchingSource`] — bridges any `BlobFetcher` into the sync [`ArtifactSource`] face
//!   the host consumes: `prefetch` (async) pulls and **verifies against the content address**
//!   before caching; `fetch` serves the verified cache. The same two-step
//!   `MeshArtifactSource` proved; the remote stays untrusted either way.
//!
//! **Two sizes, two paths (S1, `docs/plans/design-time-tooling.md` §11).** A *small* artifact
//! (a WASM component) goes through [`PrefetchingSource`] into memory, bounded by
//! [`DEFAULT_MAX_IN_MEMORY_BYTES`] and refused **by name** past it — with or without a declared
//! `Content-Length`, because the body is read in chunks and counted, never materialised first. A
//! *large* artifact (a model) goes through [`DiskStagedSource`]: pulled in HTTP `Range` pieces
//! via [`RangedBlobFetcher`], hashed as it streams, written to a node-local staging directory,
//! and served to the blob runtime from disk. Neither path holds more than one chunk in RAM
//! before the content address is checked.
//!
//! **Egress:** an object-store pull is an outbound reach the node chooses, so
//! [`HttpLibrarySource`] is gated by an [`EgressPolicy`] exactly like the LLM backends — a
//! denied host fails *before* any connection is attempted. Every pulling node carries its own
//! read credentials and its own policy: direct per-node pulls, no relay, no leader (L3).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::Arc;

use bytes::Bytes;
use mycelium::EgressPolicy;

use crate::artifact::{verify_artifact, ArtifactId, ArtifactSource, FsLibrarySource, RangedArtifactSource};

/// The most bytes the whole-object path will hold in memory for one artifact — the bound on
/// [`HttpLibrarySource::fetch_remote`] and on [`PrefetchingSource`]'s cache. 64 MiB: six times
/// the mesh frame cap, small enough that a node never OOMs before it can check a hash. Anything
/// larger is a blob and belongs to [`DiskStagedSource`].
pub const DEFAULT_MAX_IN_MEMORY_BYTES: u64 = 64 * 1024 * 1024;

/// Default HTTP `Range` piece for [`DiskStagedSource`]: 4 MiB, the blob runtime's own chunk.
pub const DEFAULT_RANGE_CHUNK_BYTES: u64 = 4 * 1024 * 1024;

/// Async remote fetch by content address — the extension point for vendor object-store SDKs.
/// `Ok(None)` = the remote doesn't hold it (a miss, not a failure); `Err` = the attempt failed
/// (unreachable, denied, 5xx) and is worth logging.
#[async_trait::async_trait]
pub trait BlobFetcher: Send + Sync {
    async fn fetch_remote(&self, id: &ArtifactId) -> Result<Option<Bytes>, String>;
}

/// Ranged remote reads by content address — the extension point a **large** artifact needs.
/// `size` answers `Ok(None)` for a miss; `fetch_range` returns at most `len` bytes from `offset`
/// (a short read at the tail is normal) and `Ok(None)` for a miss. A remote that cannot serve
/// ranges answers `Err`, never a whole body under a range request.
#[async_trait::async_trait]
pub trait RangedBlobFetcher: BlobFetcher {
    async fn size(&self, id: &ArtifactId) -> Result<Option<u64>, String>;
    async fn fetch_range(&self, id: &ArtifactId, offset: u64, len: u64) -> Result<Option<Bytes>, String>;
}

/// Bridges an async [`BlobFetcher`] into the sync [`ArtifactSource`] face: bytes must be
/// [`prefetch`](Self::prefetch)ed — pulled and verified against the content address — into the
/// cache before `WasmHost::provision` (or a serving librarian) reads them via `fetch`.
pub struct PrefetchingSource {
    fetcher:   Arc<dyn BlobFetcher>,
    cache:     Mutex<HashMap<ArtifactId, Bytes>>,
    max_bytes: u64,
}

impl PrefetchingSource {
    pub fn new(fetcher: Arc<dyn BlobFetcher>) -> Self {
        Self { fetcher, cache: Mutex::new(HashMap::new()), max_bytes: DEFAULT_MAX_IN_MEMORY_BYTES }
    }

    /// The most bytes one cached artifact may occupy (default
    /// [`DEFAULT_MAX_IN_MEMORY_BYTES`]). A larger blob is refused by name at `prefetch` —
    /// it belongs in a [`DiskStagedSource`].
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes.max(1);
        self
    }

    /// Pull `id` from the remote into the local cache, verified on arrival — a remote returning
    /// the wrong bytes is rejected like any other untrusted source. Returns whether the id is
    /// now cached. Idempotent — a cached id short-circuits.
    pub async fn prefetch(&self, id: &ArtifactId) -> bool {
        if self.cache.lock().unwrap().contains_key(id) {
            return true;
        }
        match self.fetcher.fetch_remote(id).await {
            Ok(Some(bytes)) if bytes.len() as u64 > self.max_bytes => {
                tracing::warn!(artifact = %id, bytes = bytes.len(), bound = self.max_bytes,
                    "artifact exceeds the in-memory bound — refused; stage it to disk (DiskStagedSource)");
                false
            }
            Ok(Some(bytes)) if verify_artifact(&bytes, id).is_ok() => {
                self.cache.lock().unwrap().insert(*id, bytes);
                true
            }
            Ok(Some(_)) => {
                tracing::warn!(artifact = %id, "remote returned bytes that fail the content address — rejected");
                false
            }
            Ok(None) => false,
            Err(e) => {
                tracing::warn!(artifact = %id, %e, "remote fetch failed");
                false
            }
        }
    }

    /// Prefetch a set of ids (e.g. everything in a library manifest — the mirror step for a
    /// librarian fronting a remote store). Returns how many are now cached.
    pub async fn prefetch_all(&self, ids: &[ArtifactId]) -> usize {
        let mut ok = 0;
        for id in ids {
            if self.prefetch(id).await {
                ok += 1;
            }
        }
        ok
    }
}

impl ArtifactSource for PrefetchingSource {
    fn fetch(&self, id: &ArtifactId) -> Option<Bytes> {
        self.cache.lock().unwrap().get(id).cloned()
    }
}

/// [`BlobFetcher`] over a plain HTTP(S) blob store: `GET {base_url}/{artifact-hex}`. Optional
/// static headers carry credentials (`Authorization: Bearer …`, S3-compatible static auth);
/// an [`EgressPolicy`] gates every request **before** it is dispatched.
pub struct HttpLibrarySource {
    base_url:  String,
    headers:   Vec<(String, String)>,
    egress:    EgressPolicy,
    /// Whether the node's policy was supplied ([`with_egress`](Self::with_egress)); only then are
    /// redirects followed, each hop re-checked.
    egress_set: bool,
    client:    reqwest::Client,
    max_bytes: u64,
}

/// The redirect policy for a client that knows the node's egress policy: every hop goes through
/// [`EgressPolicy::redirect_verdict`] (realignment repairs R3). This crate builds its own clients —
/// it depends on `mycelium` without `gateway` — against the same verdict `mycelium::egress_client`
/// uses.
pub(crate) fn redirect_policy(egress: EgressPolicy) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        let hop = attempt.previous().len();
        let from = attempt.previous().last().map(|u| u.scheme().to_string()).unwrap_or_default();
        let to = attempt.url();
        match egress.redirect_verdict(hop, &from, to.scheme(), to.host_str()) {
            Ok(()) => attempt.follow(),
            Err(why) => attempt.error(why),
        }
    })
}

impl HttpLibrarySource {
    /// A source reading `{base_url}/{artifact-hex}` with an allow-all egress policy (empty
    /// `allow_hosts` — [`EgressPolicy`]'s default). Production nodes should
    /// [`with_egress`](Self::with_egress) their configured policy.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            headers:  Vec::new(),
            egress:   EgressPolicy::default(),
            egress_set: false,
            client:   Self::client_for(None),
            max_bytes: DEFAULT_MAX_IN_MEMORY_BYTES,
        }
    }

    /// A client that follows **no** redirect unless the node's policy is known and no static header
    /// is attached; then every hop is re-checked. A static header may be a credential reqwest does
    /// not strip on a cross-host hop, so a source carrying one never follows (realignment repairs
    /// R3).
    fn client_for(policy: Option<&EgressPolicy>) -> reqwest::Client {
        let redirect = match policy {
            Some(p) => redirect_policy(p.clone()),
            None => reqwest::redirect::Policy::none(),
        };
        reqwest::Client::builder().redirect(redirect).build().unwrap_or_else(|_| {
            reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().expect("a plain client builds")
        })
    }

    fn rebuild_client(&mut self) {
        let policy = (self.egress_set && self.headers.is_empty()).then_some(&self.egress);
        self.client = Self::client_for(policy);
    }

    /// The most bytes [`fetch_remote`](BlobFetcher::fetch_remote) will read for one artifact
    /// (default [`DEFAULT_MAX_IN_MEMORY_BYTES`]). The body is read in pieces and counted, so
    /// the bound holds for a chunked response with no `Content-Length` too. Ranged reads
    /// ([`RangedBlobFetcher`]) are bounded by the range they ask for and ignore this.
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes.max(1);
        self
    }

    fn url_for(&self, id: &ArtifactId) -> Result<String, String> {
        let url = format!("{}/{}", self.base_url.trim_end_matches('/'), id.to_hex());
        if !self.egress.permits_url(&url) {
            return Err(format!("egress policy denies {url}"));
        }
        Ok(url)
    }

    fn request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        let mut req = self.client.request(method, url);
        for (name, value) in &self.headers {
            req = req.header(name, value);
        }
        req
    }

    /// Read a response body in pieces, refusing by name once it passes `bound` — before the
    /// piece that crosses it is kept. This is what closes E12: the old path called
    /// `resp.bytes()`, which materialises a chunked body whole before anything can be checked.
    async fn read_bounded(mut resp: reqwest::Response, bound: u64, url: &str) -> Result<Bytes, String> {
        let mut out = bytes::BytesMut::with_capacity(resp.content_length().unwrap_or(0).min(bound) as usize);
        while let Some(piece) = resp.chunk().await.map_err(|e| format!("read {url}: {e}"))? {
            if out.len() as u64 + piece.len() as u64 > bound {
                return Err(format!(
                    "GET {url}: body exceeds the in-memory bound of {bound} B (read {} B so far) — stage it to disk",
                    out.len()
                ));
            }
            out.extend_from_slice(&piece);
        }
        Ok(out.freeze())
    }

    /// Attach a static request header (credentials: `("Authorization", "Bearer …")`).
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self.rebuild_client();
        self
    }

    /// Gate pulls with the node's egress policy — the same WS3 posture as the LLM backends:
    /// an outbound pull is a reach this node chooses, and a denied host fails before any
    /// connection is attempted.
    pub fn with_egress(mut self, egress: impl Into<EgressPolicy>) -> Self {
        self.egress = egress.into();
        self.egress_set = true;
        self.rebuild_client();
        self
    }
}

#[async_trait::async_trait]
impl BlobFetcher for HttpLibrarySource {
    async fn fetch_remote(&self, id: &ArtifactId) -> Result<Option<Bytes>, String> {
        let url = self.url_for(id)?;
        let resp = self.request(reqwest::Method::GET, &url).send().await.map_err(|e| format!("GET {url}: {e}"))?;
        match resp.status() {
            s if s.is_success() => {
                // A declared length past the bound is refused before a byte is read; an
                // undeclared one is refused by `read_bounded` the moment it crosses.
                if let Some(len) = resp.content_length()
                    && len > self.max_bytes
                {
                    return Err(format!(
                        "GET {url}: declared body {len} B exceeds the in-memory bound of {} B — stage it to disk",
                        self.max_bytes
                    ));
                }
                Ok(Some(Self::read_bounded(resp, self.max_bytes, &url).await?))
            }
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            s => Err(format!("GET {url}: {s}")),
        }
    }
}

#[async_trait::async_trait]
impl RangedBlobFetcher for HttpLibrarySource {
    /// `HEAD` — the declared `Content-Length`. A store that answers without one cannot be
    /// staged in ranges, and says so.
    async fn size(&self, id: &ArtifactId) -> Result<Option<u64>, String> {
        let url = self.url_for(id)?;
        let resp = self.request(reqwest::Method::HEAD, &url).send().await.map_err(|e| format!("HEAD {url}: {e}"))?;
        match resp.status() {
            // The header, not `content_length()`: for a HEAD the body is empty by definition and
            // the client's size hint reports that emptiness, not the declared length.
            s if s.is_success() => resp
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Some)
                .ok_or_else(|| format!("HEAD {url}: no Content-Length — this store cannot be ranged")),
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            s => Err(format!("HEAD {url}: {s}")),
        }
    }

    /// `GET` with `Range: bytes=offset-(offset+len-1)`. Only a `206` is read; a `200` means the
    /// store ignored the range and would send the whole body, so the response is dropped
    /// **unread** and the call fails by name. The body is counted against `len`, never trusted.
    async fn fetch_range(&self, id: &ArtifactId, offset: u64, len: u64) -> Result<Option<Bytes>, String> {
        if len == 0 {
            return Ok(Some(Bytes::new()));
        }
        let url = self.url_for(id)?;
        let resp = self
            .request(reqwest::Method::GET, &url)
            .header(reqwest::header::RANGE, format!("bytes={}-{}", offset, offset + len - 1))
            .send()
            .await
            .map_err(|e| format!("GET {url}: {e}"))?;
        match resp.status() {
            reqwest::StatusCode::PARTIAL_CONTENT => Ok(Some(Self::read_bounded(resp, len, &url).await?)),
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            reqwest::StatusCode::OK => Err(format!(
                "GET {url}: the store ignored the Range header (answered 200) — refusing to read a whole body"
            )),
            s => Err(format!("GET {url} (range {offset}+{len}): {s}")),
        }
    }
}

/// The **large-artifact** path: pull a blob from a [`RangedBlobFetcher`] in range-sized pieces
/// into a node-local staging directory, hashing as it streams, and serve it from disk. Peak
/// memory is one piece, whatever the blob's size. The staged file is complete-or-absent: pieces
/// land in a uniquely named `.part-…` file that is renamed into place only after the content
/// address matches, so a reader (the blob runtime's own ranged install, a librarian mirroring a
/// remote store) never sees a partial or unverified blob. Staging the same id twice is idempotent.
///
/// Serving is delegated to an [`FsLibrarySource`] over the same directory, so everything a
/// library directory offers — ranged reads, `list`, `remove` — is available on the stage.
pub struct DiskStagedSource {
    fetcher:         Arc<dyn RangedBlobFetcher>,
    stage:           FsLibrarySource,
    chunk_bytes:     u64,
    /// The most one artifact may stage, in bytes. A holder's `size` reply is a claim from an
    /// untrusted party; past this it is refused before a byte is requested.
    max_stage_bytes: u64,
}

/// The most one artifact stages by default: 64 GiB — above the largest model blob the stem
/// examples place, and a bound rather than none, so a holder answering `artifact.size` with
/// `u64::MAX` is refused before the stage writes toward it. `DiskStagedSource::with_max_stage_bytes`
/// and `StemOptions::max_stage_bytes` set it.
pub const DEFAULT_MAX_STAGE_BYTES: u64 = 64 * 1024 * 1024 * 1024;

impl DiskStagedSource {
    /// A staged source over `dir` (created if absent), pulling in [`DEFAULT_RANGE_CHUNK_BYTES`]
    /// pieces, each artifact bounded by [`DEFAULT_MAX_STAGE_BYTES`].
    pub fn open(fetcher: Arc<dyn RangedBlobFetcher>, dir: impl Into<std::path::PathBuf>) -> std::io::Result<Self> {
        Ok(Self {
            fetcher,
            stage: FsLibrarySource::open(dir)?,
            chunk_bytes: DEFAULT_RANGE_CHUNK_BYTES,
            max_stage_bytes: DEFAULT_MAX_STAGE_BYTES,
        })
    }

    /// The most one artifact may stage. A `size` reply above it is refused before any range is
    /// requested.
    pub fn with_max_stage_bytes(mut self, max_stage_bytes: u64) -> Self {
        self.max_stage_bytes = max_stage_bytes;
        self
    }

    /// Override the range piece (min 1; tests use tiny pieces to exercise many rounds).
    pub fn with_chunk_bytes(mut self, chunk_bytes: u64) -> Self {
        self.chunk_bytes = chunk_bytes.max(1);
        self
    }

    /// The staging directory, as a library source.
    pub fn stage(&self) -> &FsLibrarySource {
        &self.stage
    }

    /// Pull `id` into the stage, verified, returning whether it is now staged. A miss, a store
    /// that cannot be ranged, a range the store ignores, a short or failed piece, a size past
    /// the stage's ceiling, or a hash mismatch each return `false` with the reason logged;
    /// nothing partial is left behind. Idempotent — a staged id short-circuits without a request.
    pub async fn stage_artifact(&self, id: &ArtifactId) -> bool {
        self.stage_artifact_bounded(id, 0).await
    }

    /// [`stage_artifact`](Self::stage_artifact) under the catalogue entry's own size hint as well:
    /// `size_hint` (0 = none) is the entry's `size_bytes`, and a holder's `size` reply above it —
    /// or above the stage's ceiling — is refused **before any range is requested**. The hint is
    /// a ranking hint outside the signature, so it bounds the pull without being trusted for
    /// anything else: the hash still decides what is kept.
    pub async fn stage_artifact_bounded(&self, id: &ArtifactId, size_hint: u64) -> bool {
        use sha2::{Digest, Sha256};
        use tokio::io::AsyncWriteExt;

        if self.stage.size(id).is_some() {
            return true;
        }
        let total = match self.fetcher.size(id).await {
            Ok(Some(n)) => n,
            Ok(None) => return false,
            Err(e) => {
                tracing::warn!(artifact = %id, %e, "cannot stage: size unknown");
                return false;
            }
        };
        let ceiling = if size_hint > 0 { size_hint.min(self.max_stage_bytes) } else { self.max_stage_bytes };
        if total > ceiling {
            tracing::warn!(artifact = %id, total, ceiling, size_hint, max_stage_bytes = self.max_stage_bytes,
                "cannot stage: the holder's size reply is past the ceiling — refused before any byte");
            metrics::counter!("mycelium_artifact_stage_refused_total", "reason" => "size_past_ceiling").increment(1);
            return false;
        }
        static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let tmp = self.stage.dir().join(format!(
            ".part-{}-{}-{}",
            id.to_hex(),
            std::process::id(),
            TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        let result: Result<(), String> = async {
            let mut file = tokio::fs::File::create(&tmp).await.map_err(|e| format!("create {}: {e}", tmp.display()))?;
            let mut hasher = Sha256::new();
            let mut fetched = 0u64;
            while fetched < total {
                let want = self.chunk_bytes.min(total - fetched);
                let piece = match self.fetcher.fetch_range(id, fetched, want).await? {
                    Some(p) => p,
                    None => return Err("the store stopped holding the artifact mid-pull".into()),
                };
                if piece.is_empty() {
                    return Err(format!("empty range at {fetched}/{total}"));
                }
                hasher.update(&piece);
                file.write_all(&piece).await.map_err(|e| format!("write {}: {e}", tmp.display()))?;
                fetched += piece.len() as u64;
            }
            file.flush().await.map_err(|e| format!("flush {}: {e}", tmp.display()))?;
            drop(file);
            let actual = ArtifactId::from_hex(&format!("{:x}", hasher.finalize())).map_err(|e| e.to_string())?;
            if actual != *id {
                return Err(format!("artifact hash mismatch: expected {id}, got {actual}"));
            }
            Ok(())
        }
        .await;
        match result {
            Ok(()) => match tokio::fs::rename(&tmp, self.stage.dir().join(id.to_hex())).await {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!(artifact = %id, %e, "staged bytes verified but could not be placed");
                    let _ = tokio::fs::remove_file(&tmp).await;
                    false
                }
            },
            Err(e) => {
                tracing::warn!(artifact = %id, %e, "staging failed — nothing kept");
                let _ = tokio::fs::remove_file(&tmp).await;
                false
            }
        }
    }

    /// Stage a set of ids (a manifest's worth — the mirror step for a librarian fronting a
    /// remote store). Returns how many are now staged.
    pub async fn stage_all(&self, ids: &[ArtifactId]) -> usize {
        let mut ok = 0;
        for id in ids {
            if self.stage_artifact(id).await {
                ok += 1;
            }
        }
        ok
    }
}

impl ArtifactSource for DiskStagedSource {
    /// Whole-blob read from the stage — for a small staged artifact; a large one should be read
    /// through [`as_ranged`](ArtifactSource::as_ranged), as the blob runtime does.
    fn fetch(&self, id: &ArtifactId) -> Option<Bytes> {
        self.stage.fetch(id)
    }

    fn as_ranged(&self) -> Option<&dyn RangedArtifactSource> {
        Some(self)
    }
}

impl RangedArtifactSource for DiskStagedSource {
    fn size(&self, id: &ArtifactId) -> Option<u64> {
        self.stage.size(id)
    }

    fn fetch_range(&self, id: &ArtifactId, offset: u64, len: u64) -> Option<Bytes> {
        self.stage.fetch_range(id, offset, len)
    }
}

#[cfg(test)]
mod tests {
    /// `with_egress` takes the node's policy by reference, as `agent.egress_policy()` hands it, or by
    /// value (doc-coverage run 20, code gap 5).
    #[test]
    fn with_egress_takes_the_policy_by_reference_or_by_value() {
        let policy = mycelium::EgressPolicy { allow_hosts: vec!["artifacts.internal".into()] };
        let _ = HttpLibrarySource::new("http://artifacts.internal").with_egress(&policy);
        let _ = HttpLibrarySource::new("http://artifacts.internal").with_egress(policy.clone());
    }

    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A minimal blocking HTTP file server: serves `GET /{hex}` from a map, counts hits,
    /// records the Authorization header it saw. Enough protocol for reqwest; no deps.
    fn spawn_test_server(
        blobs: HashMap<String, Vec<u8>>,
    ) -> (String, Arc<AtomicUsize>, Arc<Mutex<Option<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let auth_seen = Arc::new(Mutex::new(None));
        let (h, a) = (Arc::clone(&hits), Arc::clone(&auth_seen));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                h.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.lines().next().and_then(|l| l.split(' ').nth(1)).unwrap_or("/");
                if let Some(line) = req.lines().find(|l| l.to_ascii_lowercase().starts_with("authorization:")) {
                    *a.lock().unwrap() =
                        Some(line.split_once(' ').map(|x| x.1).unwrap_or("").trim().to_string());
                }
                let key = path.trim_start_matches('/');
                let response = match blobs.get(key) {
                    Some(body) => {
                        let mut r = format!(
                            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        r.extend_from_slice(body);
                        r
                    }
                    None => b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
                };
                let _ = stream.write_all(&response);
            }
        });
        (format!("http://{addr}"), hits, auth_seen)
    }


    // ── S1 fixtures: a range-capable, optionally chunked, on-the-fly blob server ─────────────
    // (`docs/plans/design-time-tooling.md` §11 S1). The body is generated per offset, never
    // materialised, so the server can serve gigabytes without holding them.

    fn gen_byte(i: u64) -> u8 {
        ((i.wrapping_mul(2_654_435_761) ^ (i >> 7)) & 0xff) as u8
    }

    fn gen_id(len: u64) -> ArtifactId {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut off = 0u64;
        while off < len {
            let n = (buf.len() as u64).min(len - off) as usize;
            for (k, b) in buf[..n].iter_mut().enumerate() {
                *b = gen_byte(off + k as u64);
            }
            h.update(&buf[..n]);
            off += n as u64;
        }
        ArtifactId::from_hex(&format!("{:x}", h.finalize())).unwrap()
    }

    #[derive(Clone, Copy)]
    enum ServeMode {
        /// `Content-Length` on GET, `206` + `Content-Range` on a `Range` request, `HEAD` answered.
        Ranged,
        /// `Transfer-Encoding: chunked`, no length, `Range` ignored (a CDN that streams).
        ChunkedNoLength,
        /// Declares a length but ignores `Range` (answers `200` with the whole body).
        IgnoresRange,
    }

    /// Serve one blob of `len` generated bytes at `/{hex}` for any hex; `lie` flips every byte
    /// so the content address never matches.
    fn spawn_range_server(len: u64, mode: ServeMode, lie: bool) -> (String, Arc<AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = Arc::clone(&hits);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                h.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let method = req.split(' ').next().unwrap_or("GET").to_string();
                let range = req.lines().find_map(|l| {
                    let l = l.trim();
                    l.to_ascii_lowercase().strip_prefix("range: bytes=").map(|r| {
                        let (a, b) = r.split_once('-').unwrap();
                        let a: u64 = a.parse().unwrap();
                        let b: u64 = b.parse().unwrap_or(len - 1);
                        (a, b.min(len - 1))
                    })
                });
                let write_body = |stream: &mut std::net::TcpStream, from: u64, to_incl: u64, chunked: bool| {
                    let mut off = from;
                    let mut piece = vec![0u8; 1 << 16];
                    while off <= to_incl {
                        let n = (piece.len() as u64).min(to_incl - off + 1) as usize;
                        for (k, b) in piece[..n].iter_mut().enumerate() {
                            let v = gen_byte(off + k as u64);
                            *b = if lie { !v } else { v };
                        }
                        if chunked {
                            let _ = stream.write_all(format!("{n:x}\r\n").as_bytes());
                            let _ = stream.write_all(&piece[..n]);
                            let _ = stream.write_all(b"\r\n");
                        } else {
                            let _ = stream.write_all(&piece[..n]);
                        }
                        off += n as u64;
                    }
                    if chunked {
                        let _ = stream.write_all(b"0\r\n\r\n");
                    }
                };
                match (mode, method.as_str(), range) {
                    (ServeMode::Ranged, "HEAD", _) | (ServeMode::IgnoresRange, "HEAD", _) => {
                        let _ = stream.write_all(
                            format!("HTTP/1.1 200 OK\r\ncontent-length: {len}\r\naccept-ranges: bytes\r\nconnection: close\r\n\r\n").as_bytes(),
                        );
                    }
                    (ServeMode::ChunkedNoLength, "HEAD", _) => {
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n");
                    }
                    (ServeMode::Ranged, _, Some((a, b))) => {
                        let _ = stream.write_all(
                            format!("HTTP/1.1 206 Partial Content\r\ncontent-length: {}\r\ncontent-range: bytes {a}-{b}/{len}\r\nconnection: close\r\n\r\n", b - a + 1).as_bytes(),
                        );
                        write_body(&mut stream, a, b, false);
                    }
                    (ServeMode::Ranged, _, None) | (ServeMode::IgnoresRange, _, _) => {
                        let _ = stream.write_all(
                            format!("HTTP/1.1 200 OK\r\ncontent-length: {len}\r\nconnection: close\r\n\r\n").as_bytes(),
                        );
                        write_body(&mut stream, 0, len - 1, false);
                    }
                    (ServeMode::ChunkedNoLength, _, _) => {
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n");
                        write_body(&mut stream, 0, len - 1, true);
                    }
                }
            }
        });
        (format!("http://{addr}"), hits)
    }

    const MIB: u64 = 1024 * 1024;

    /// E12's residual, pinned: a chunked response with no `Content-Length` was read whole. The
    /// whole-object path must refuse **by name** once the body passes the in-memory bound, with
    /// or without a declared length.
    ///
    /// Seen failing first at the default bound (a 65 MiB chunked body materialised whole);
    /// kept at a 2 MiB bound so the unit test does not itself hold 64 MiB.
    #[tokio::test]
    async fn a_chunked_body_with_no_length_is_bounded_by_name() {
        let len = 3 * MIB; // one past the bound set below
        let (base, _hits) = spawn_range_server(len, ServeMode::ChunkedNoLength, false);
        let id = gen_id(len);
        let err = match HttpLibrarySource::new(&base).with_max_bytes(2 * MIB).fetch_remote(&id).await {
            Err(e) => e,
            Ok(b) => panic!(
                "a body past the in-memory bound must be refused, not materialised: got {} B",
                b.map(|b| b.len()).unwrap_or(0)
            ),
        };
        assert!(err.contains("exceeds") && err.contains("bound"), "refused by name, got: {err}");
    }

    /// The prefetch cache is for small artifacts. A blob past its bound is refused by name —
    /// never silently held in RAM — even when the store declares a length under the old cap.
    #[tokio::test]
    async fn the_prefetch_cache_refuses_a_blob_past_its_bound() {
        let len = 65 * MIB;
        let (base, _hits) = spawn_range_server(len, ServeMode::Ranged, false);
        let id = gen_id(len);
        let source = PrefetchingSource::new(Arc::new(HttpLibrarySource::new(&base)));
        assert!(!source.prefetch(&id).await, "65 MiB must not enter the in-memory cache");
        assert!(source.fetch(&id).is_none());
    }

    fn rss_kib() -> u64 {
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .expect("ps");
        String::from_utf8_lossy(&out.stdout).trim().parse().expect("rss")
    }

    /// S1's exit gate: a blob far larger than the in-memory bound stages to disk in range
    /// pieces with peak resident memory bounded — the size is `MYCELIUM_S1_BLOB_MIB` (default
    /// 64; the plan's 2 GiB run is the same test with the variable set), the bound is
    /// size-independent because only one piece is ever held.
    ///
    /// Ignored by default because it measures the **process's** resident memory, which the
    /// other tests in this binary perturb when they run beside it (a chunked-body test holding
    /// its bound, a wasm instantiation): it must own the process. CI runs it alone:
    /// `cargo test -p mycelium-wasm-host --lib a_large_blob_stages -- --ignored`.
    #[tokio::test]
    #[ignore = "measures process RSS; run alone: cargo test -p mycelium-wasm-host --lib a_large_blob_stages -- --ignored"]
    async fn a_large_blob_stages_to_disk_within_a_memory_bound() {
        let mib: u64 = std::env::var("MYCELIUM_S1_BLOB_MIB").ok().and_then(|v| v.parse().ok()).unwrap_or(64);
        let len = mib * MIB;
        let (base, hits) = spawn_range_server(len, ServeMode::Ranged, false);
        let id = gen_id(len);
        let dir = std::env::temp_dir().join(format!("mycelium-s1-stage-{}-{}", std::process::id(), mib));
        let _ = std::fs::remove_dir_all(&dir);
        let staged = DiskStagedSource::open(Arc::new(HttpLibrarySource::new(&base)), &dir)
            .unwrap()
            .with_chunk_bytes(4 * MIB);

        let before = rss_kib();
        assert!(staged.stage_artifact(&id).await, "the blob stages");
        let after = rss_kib();
        let grew_mib = after.saturating_sub(before) / 1024;
        assert!(grew_mib < 32, "peak RSS grew by {grew_mib} MiB staging {mib} MiB — the pull is not streaming");
        assert!(hits.load(Ordering::SeqCst) as u64 >= len / (4 * MIB), "one request per 4 MiB piece");

        // Served from disk, ranged — the shape the blob runtime's install consumes.
        assert_eq!(staged.as_ranged().unwrap().size(&id), Some(len));
        let tail = staged.fetch_range(&id, len - 5, 5).unwrap();
        assert_eq!(tail.len(), 5);
        assert_eq!(tail[4], gen_byte(len - 1));
        assert!(dir.join(id.to_hex()).exists(), "complete-or-absent: the verified file is in place");
        assert!(!std::fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".part-")),
            "no partial file left behind");
        // Idempotent: a second stage makes no request.
        let h = hits.load(Ordering::SeqCst);
        assert!(staged.stage_artifact(&id).await);
        assert_eq!(hits.load(Ordering::SeqCst), h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The staging pull is bounded before its first byte: a peer that answers `artifact.size`
    /// with `u64::MAX` (or any size past the entry's hint or the stage's ceiling) is refused with
    /// no range requested and no partial file created — the size reply is a hint from an untrusted
    /// holder, and the stage must not write toward it. Seen failing first: the stage created its
    /// `.part` file and asked for the first range.
    #[tokio::test]
    async fn a_size_reply_past_the_ceiling_is_refused_before_any_byte_is_written() {
        struct Liar { ranges: AtomicUsize }
        #[async_trait::async_trait]
        impl BlobFetcher for Liar {
            async fn fetch_remote(&self, _id: &ArtifactId) -> Result<Option<Bytes>, String> { Ok(None) }
        }
        #[async_trait::async_trait]
        impl RangedBlobFetcher for Liar {
            async fn size(&self, _id: &ArtifactId) -> Result<Option<u64>, String> { Ok(Some(u64::MAX)) }
            async fn fetch_range(&self, _id: &ArtifactId, _o: u64, _l: u64) -> Result<Option<Bytes>, String> {
                // Count the request, then end the pull: on the unfixed code the stage would
                // otherwise write toward u64::MAX one piece at a time.
                self.ranges.fetch_add(1, Ordering::SeqCst);
                Err("the holder went away".into())
            }
        }
        let liar = Arc::new(Liar { ranges: AtomicUsize::new(0) });
        let dir = std::env::temp_dir().join(format!("mycelium-stage-ceiling-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let staged = DiskStagedSource::open(Arc::clone(&liar) as Arc<dyn RangedBlobFetcher>, &dir).unwrap();
        let id = ArtifactId::of(b"whatever the catalogue named");

        // The stage's own ceiling (the default) refuses u64::MAX...
        assert!(!staged.stage_artifact(&id).await, "a size of u64::MAX must not stage");
        assert_eq!(liar.ranges.load(Ordering::SeqCst), 0, "no range was requested");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "no partial file was created");

        // ...and the entry's own size hint is a tighter ceiling than the stage's: a holder
        // claiming more than the catalogue line said is refused the same way.
        struct Inflated;
        #[async_trait::async_trait]
        impl BlobFetcher for Inflated {
            async fn fetch_remote(&self, _id: &ArtifactId) -> Result<Option<Bytes>, String> { Ok(None) }
        }
        #[async_trait::async_trait]
        impl RangedBlobFetcher for Inflated {
            async fn size(&self, _id: &ArtifactId) -> Result<Option<u64>, String> { Ok(Some(4096)) }
            async fn fetch_range(&self, _id: &ArtifactId, _o: u64, _l: u64) -> Result<Option<Bytes>, String> {
                panic!("a range was requested past the entry's size hint")
            }
        }
        let staged = DiskStagedSource::open(Arc::new(Inflated), &dir).unwrap();
        assert!(!staged.stage_artifact_bounded(&id, 1024).await, "4096 claimed against a 1024 hint");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        // A tighter stage ceiling than the hint is refused too.
        let staged = DiskStagedSource::open(Arc::new(Inflated), &dir).unwrap().with_max_stage_bytes(2048);
        assert!(!staged.stage_artifact_bounded(&id, 8192).await);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The stage keeps nothing it could not verify or could not bound: a lying store, a store
    /// that ignores `Range` (a `200` is dropped unread), and a store with no length each leave
    /// the directory empty.
    #[tokio::test]
    async fn the_stage_keeps_nothing_it_cannot_verify_or_bound() {
        let len = 3 * MIB;
        let id = gen_id(len);
        for (mode, lie, why) in [
            (ServeMode::Ranged, true, "lying bytes"),
            (ServeMode::IgnoresRange, false, "range ignored"),
            (ServeMode::ChunkedNoLength, false, "no length"),
        ] {
            let (base, _hits) = spawn_range_server(len, mode, lie);
            let dir = std::env::temp_dir().join(format!("mycelium-s1-refuse-{}-{why}", std::process::id()).replace(' ', "-"));
            let _ = std::fs::remove_dir_all(&dir);
            let staged = DiskStagedSource::open(Arc::new(HttpLibrarySource::new(&base)), &dir)
                .unwrap()
                .with_chunk_bytes(MIB);
            assert!(!staged.stage_artifact(&id).await, "{why}: must not stage");
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "{why}: nothing kept");
            assert!(staged.fetch(&id).is_none());
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[tokio::test]
    async fn http_source_pulls_verifies_and_serves_via_the_prefetch_cache() {
        let good = b"the artifact the catalogue promised".to_vec();
        let good_id = ArtifactId::of(&good);
        let lying = b"advertised under someone else's hash".to_vec();
        let lied_about = ArtifactId::of(b"what the catalogue actually named");

        let mut blobs = HashMap::new();
        blobs.insert(good_id.to_hex(), good.clone());
        blobs.insert(lied_about.to_hex(), lying); // wrong bytes at that key
        let (base, _hits, auth) = spawn_test_server(blobs);

        let source = PrefetchingSource::new(Arc::new(
            HttpLibrarySource::new(&base).with_header("Authorization", "Bearer test-token"),
        ));

        // Happy path: pulled over HTTP, verified, then served through the sync face.
        assert!(source.prefetch(&good_id).await, "pull + verify from the HTTP store");
        assert_eq!(source.fetch(&good_id).as_deref(), Some(&good[..]));
        assert_eq!(auth.lock().unwrap().as_deref(), Some("Bearer test-token"),
            "credentials header reaches the store");

        // A lying store is rejected (content address is the trust anchor)…
        assert!(!source.prefetch(&lied_about).await, "wrong bytes are rejected, not cached");
        assert!(source.fetch(&lied_about).is_none());

        // …and a miss is a miss.
        assert!(!source.prefetch(&ArtifactId::of(b"nobody has this")).await);

        // prefetch_all mirrors a manifest's worth in one call (cached ids short-circuit).
        assert_eq!(source.prefetch_all(&[good_id, lied_about]).await, 1);
    }

    #[tokio::test]
    async fn egress_policy_denies_before_any_connection() {
        let (base, hits, _auth) = spawn_test_server(HashMap::new());

        // 127.0.0.1 is not in the allowlist → denied *before* dispatch.
        let gated = HttpLibrarySource::new(&base)
            .with_egress(EgressPolicy { allow_hosts: vec!["library.allowed.example".into()] });
        let id = ArtifactId::of(b"x");
        let err = gated.fetch_remote(&id).await.expect_err("denied host errors");
        assert!(err.contains("egress policy denies"), "got: {err}");
        assert_eq!(hits.load(Ordering::SeqCst), 0, "no connection was attempted");

        // The prefetching wrapper surfaces it as a non-cache (logged), not a panic.
        let source = PrefetchingSource::new(Arc::new(gated));
        assert!(!source.prefetch(&id).await);
        assert_eq!(hits.load(Ordering::SeqCst), 0);

        // An allow-all policy (the default) reaches the store.
        let open = HttpLibrarySource::new(&base);
        assert_eq!(open.fetch_remote(&id).await.unwrap(), None, "404 is a miss");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    /// **Realignment repairs R3.** A source without the node's policy follows no redirect; one
    /// with it re-checks every hop; and one carrying a static header never follows, because the
    /// header may be a credential reqwest does not strip cross-host.
    #[tokio::test]
    async fn the_http_source_follows_a_redirect_only_with_a_policy_and_no_header() {
        use mycelium::test_util::{spawn_counting_listener, spawn_redirector};
        let fetch = |src: HttpLibrarySource| async move { src.fetch_remote(&ArtifactId::of(b"blob")).await };

        let (denied, hits) = spawn_counting_listener("blob").await;
        let hop = spawn_redirector(302, format!("http://127.0.0.1:{denied}")).await;
        let _ = fetch(HttpLibrarySource::new(format!("http://localhost:{hop}"))).await;
        assert_eq!(hits.load(Ordering::SeqCst), 0, "no policy, no redirect");

        let one = EgressPolicy { allow_hosts: vec!["localhost".into()] };
        let _ = fetch(HttpLibrarySource::new(format!("http://localhost:{hop}")).with_egress(one)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 0, "a denied hop is refused");

        let both = EgressPolicy { allow_hosts: vec!["localhost".into(), "127.0.0.1".into()] };
        let _ = fetch(HttpLibrarySource::new(format!("http://localhost:{hop}")).with_egress(both.clone())).await;
        assert!(hits.load(Ordering::SeqCst) > 0, "the plant: an allowed hop is followed");

        let (elsewhere, cred_hits) = spawn_counting_listener("blob").await;
        let hop = spawn_redirector(302, format!("http://127.0.0.1:{elsewhere}")).await;
        let src = HttpLibrarySource::new(format!("http://localhost:{hop}")).with_egress(both).with_header("Authorization", "Bearer secret");
        let _ = fetch(src).await;
        assert_eq!(cred_hits.load(Ordering::SeqCst), 0, "a source carrying a header never follows");
    }
}
