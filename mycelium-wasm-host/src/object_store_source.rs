//! The **object-store adapter** — S3, GCS, Azure, plain HTTP and local, through one crate
//! (`docs/plans/design-time-tooling.md` §11, D11–D14; phases S2/S3).
//!
//! [`ObjectStoreFetcher`] wraps an `object_store` store selected by URL — `s3://bucket/prefix`,
//! `gs://bucket/prefix`, `az://…`, `https://host/prefix`, `file:///dir` — and implements both
//! [`BlobFetcher`] (whole-object, bounded by the in-memory bound) and [`RangedBlobFetcher`]
//! (`head` for the size, `get_range` for a piece), so a large artifact stages to disk through
//! [`DiskStagedSource`](crate::DiskStagedSource) in pieces exactly as it does from HTTP.
//!
//! **Credentials are the node's cloud identity** (D13): the builders read the environment —
//! `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` or an instance/task role, `GOOGLE_SERVICE_ACCOUNT`
//! or the metadata server, `AWS_ENDPOINT` + `AWS_ALLOW_HTTP=true` for an S3-compatible store
//! such as MinIO — never a unit file, a description or a manifest. Every request is gated by the
//! node's [`EgressPolicy`] on the store's URL **before** a client is built (a bucket host outside
//! `allow_hosts` never gets a connection).
//!
//! **The manifest lives in the store too** (D14): `<prefix>/manifest`, the same line-hex text a
//! library directory carries, so a librarian fronting a remote store reads it from the store
//! ([`ManifestSource`]) and `mycelium-artifact publish --library s3://…` writes blob and manifest
//! through the same adapter.

use std::sync::Arc;

use bytes::Bytes;
use mycelium::EgressPolicy;
use object_store::path::Path as StorePath;
use object_store::{parse_url_opts, DynObjectStore, Error as StoreError, ObjectStoreExt, PutPayload};

use crate::artifact::ArtifactId;
use crate::catalog::{Manifest, ManifestError, MANIFEST_FILE};
use crate::http_source::{BlobFetcher, RangedBlobFetcher, DEFAULT_MAX_IN_MEMORY_BYTES};
use crate::librarian::ManifestSource;

/// A content-addressed library in an object store: blobs at `<prefix>/<hex>`, the manifest at
/// `<prefix>/manifest`.
pub struct ObjectStoreFetcher {
    store:     Arc<DynObjectStore>,
    prefix:    StorePath,
    url:       String,
    egress:    EgressPolicy,
    max_bytes: u64,
}

impl std::fmt::Debug for ObjectStoreFetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectStoreFetcher").field("url", &self.url).finish()
    }
}

fn store_err(what: &str, e: StoreError) -> String {
    format!("{what}: {e}")
}

impl ObjectStoreFetcher {
    /// Open the store `url` names, with the builder options taken from the process environment.
    /// Refused **before** a client is built when `egress` does not permit the URL.
    pub fn from_url(url: &str, egress: EgressPolicy) -> Result<Self, String> {
        if !egress.permits_url(url) {
            return Err(format!("egress policy denies {url}"));
        }
        let parsed = reqwest::Url::parse(url).map_err(|e| format!("{url}: {e}"))?;
        let (store, prefix) = parse_url_opts(&parsed, std::env::vars()).map_err(|e| format!("{url}: {e}"))?;
        Ok(Self { store: Arc::from(store), prefix, url: url.to_string(), egress, max_bytes: DEFAULT_MAX_IN_MEMORY_BYTES })
    }

    /// The most bytes [`fetch_remote`](BlobFetcher::fetch_remote) will hold for one artifact.
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes.max(1);
        self
    }

    /// The URL this store was opened from.
    pub fn url(&self) -> &str {
        &self.url
    }

    fn gate(&self) -> Result<(), String> {
        if self.egress.permits_url(&self.url) { Ok(()) } else { Err(format!("egress policy denies {}", self.url)) }
    }

    fn location(&self, id: &ArtifactId) -> StorePath {
        self.prefix.clone().join(id.to_hex())
    }

    fn manifest_location(&self) -> StorePath {
        self.prefix.clone().join(MANIFEST_FILE)
    }

    /// Store `bytes` content-addressed; returns their id. Idempotent: the same bytes land at the
    /// same key.
    pub async fn put_blob(&self, bytes: Bytes) -> Result<ArtifactId, String> {
        self.gate()?;
        let id = ArtifactId::of(&bytes);
        self.store
            .put(&self.location(&id), PutPayload::from(bytes))
            .await
            .map_err(|e| store_err("put", e))?;
        Ok(id)
    }

    /// The manifest at `<prefix>/manifest`; absent = an empty manifest (a new library).
    pub async fn read_manifest(&self) -> Result<Manifest, ManifestError> {
        self.gate().map_err(|e| ManifestError::Io(std::io::Error::other(e)))?;
        match self.store.get(&self.manifest_location()).await {
            Ok(r) => {
                let bytes = r.bytes().await.map_err(|e| ManifestError::Io(std::io::Error::other(e.to_string())))?;
                Manifest::parse(&String::from_utf8_lossy(&bytes))
            }
            Err(StoreError::NotFound { .. }) => Ok(Manifest::new()),
            Err(e) => Err(ManifestError::Io(std::io::Error::other(e.to_string()))),
        }
    }

    /// Write the manifest to `<prefix>/manifest` (an object put is atomic on every backend).
    pub async fn write_manifest(&self, manifest: &Manifest) -> Result<(), String> {
        self.gate()?;
        self.store
            .put(&self.manifest_location(), PutPayload::from(Bytes::from(manifest.render())))
            .await
            .map_err(|e| store_err("put manifest", e))?;
        Ok(())
    }

    /// Hash an object by streaming it in `chunk` pieces — never materialised — and compare with
    /// its address. `Ok(None)` for a miss.
    pub async fn hash_matches(&self, id: &ArtifactId, chunk: u64) -> Result<Option<bool>, String> {
        use sha2::{Digest, Sha256};
        let Some(total) = self.size(id).await? else { return Ok(None) };
        let mut h = Sha256::new();
        let mut off = 0u64;
        while off < total {
            let want = chunk.max(1).min(total - off);
            let Some(piece) = self.fetch_range(id, off, want).await? else { return Ok(None) };
            if piece.is_empty() {
                return Err(format!("empty range at {off}/{total}"));
            }
            h.update(&piece);
            off += piece.len() as u64;
        }
        let actual = ArtifactId::from_hex(&format!("{:x}", h.finalize())).map_err(|e| e.to_string())?;
        Ok(Some(actual == *id))
    }
}

#[async_trait::async_trait]
impl BlobFetcher for ObjectStoreFetcher {
    /// Whole-object read, bounded: the size is checked first and a larger object is refused by
    /// name — it belongs to the ranged path.
    async fn fetch_remote(&self, id: &ArtifactId) -> Result<Option<Bytes>, String> {
        self.gate()?;
        let Some(size) = self.size(id).await? else { return Ok(None) };
        if size > self.max_bytes {
            return Err(format!(
                "{}: object is {size} B, past the in-memory bound of {} B — stage it to disk",
                id, self.max_bytes
            ));
        }
        match self.store.get(&self.location(id)).await {
            Ok(r) => Ok(Some(r.bytes().await.map_err(|e| store_err("read", e))?)),
            Err(StoreError::NotFound { .. }) => Ok(None),
            Err(e) => Err(store_err("get", e)),
        }
    }
}

#[async_trait::async_trait]
impl RangedBlobFetcher for ObjectStoreFetcher {
    async fn size(&self, id: &ArtifactId) -> Result<Option<u64>, String> {
        self.gate()?;
        match self.store.head(&self.location(id)).await {
            Ok(meta) => Ok(Some(meta.size)),
            Err(StoreError::NotFound { .. }) => Ok(None),
            Err(e) => Err(store_err("head", e)),
        }
    }

    async fn fetch_range(&self, id: &ArtifactId, offset: u64, len: u64) -> Result<Option<Bytes>, String> {
        self.gate()?;
        if len == 0 {
            return Ok(Some(Bytes::new()));
        }
        match self.store.get_range(&self.location(id), offset..offset + len).await {
            Ok(b) => Ok(Some(b)),
            Err(StoreError::NotFound { .. }) => Ok(None),
            Err(e) => Err(store_err("get_range", e)),
        }
    }
}

#[async_trait::async_trait]
impl ManifestSource for ObjectStoreFetcher {
    async fn load_manifest(&self) -> Result<Manifest, ManifestError> {
        self.read_manifest().await
    }
}

// ── the artifact tool over a store ───────────────────────────────────────────────────────────

/// `mycelium-artifact publish` against a store URL: read the description and its bytes, put the
/// blob, build and sign the entry, upsert it into the store's manifest.
pub async fn publish_to_store(
    description: &std::path::Path,
    fetcher: &ObjectStoreFetcher,
    key: &ed25519_dalek::SigningKey,
) -> Result<crate::tools::PublishOutcome, String> {
    let (d, bytes) = crate::tools::read_description_and_bytes(description)?;
    let artifact = fetcher.put_blob(Bytes::from(bytes.clone())).await?;
    let entry = crate::tools::build_entry(&d, &bytes, artifact, key)?;
    let mut manifest = fetcher.read_manifest().await.map_err(|e| format!("manifest: {e}"))?;
    manifest.upsert(entry.clone());
    fetcher.write_manifest(&manifest).await?;
    Ok(crate::tools::PublishOutcome {
        artifact,
        provides: entry.provides,
        kind: entry.kind,
        size_bytes: bytes.len() as u64,
        signer: format!("ed25519:{}", key.verifying_key().to_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>()),
        proposed: d.proposed,
    })
}

/// [`crate::tools::accept`] against a store: read the manifest, accept the one entry `selector`
/// names, write the manifest back (whole-object last-write-wins, like publish).
pub async fn accept_in_store(
    fetcher: &ObjectStoreFetcher,
    selector: &str,
    reviewer: &ed25519_dalek::SigningKey,
) -> Result<crate::tools::AcceptOutcome, String> {
    let mut m = fetcher.read_manifest().await.map_err(|e| format!("manifest: {e}"))?;
    let entry = crate::tools::select_entry(&m, selector)?.clone();
    if entry.acceptance.is_some() {
        return Err(format!("{}/{} ({}) is already accepted", entry.provides.namespace, entry.provides.name, &entry.artifact.to_hex()[..12]));
    }
    let accepted = entry.accepted_by(reviewer)?;
    let hex = |b: &[u8]| b.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let out = crate::tools::AcceptOutcome {
        artifact: accepted.artifact,
        provides: accepted.provides.clone(),
        signer: format!("ed25519:{}", hex(&accepted.signer)),
        reviewer: format!("ed25519:{}", hex(&reviewer.verifying_key().to_bytes())),
    };
    m.upsert(accepted);
    fetcher.write_manifest(&m).await?;
    Ok(out)
}

/// `mycelium-artifact list` against a store URL.
pub async fn list_store(fetcher: &ObjectStoreFetcher) -> Result<String, String> {
    let m = fetcher.read_manifest().await.map_err(|e| format!("manifest: {e}"))?;
    Ok(m.entries().iter().map(crate::tools::render_entry).collect::<Vec<_>>().join("\n"))
}

/// `mycelium-artifact verify` against a store URL: provenance, presence, and a streamed hash.
pub async fn verify_store(
    fetcher: &ObjectStoreFetcher,
    trusted: &[[u8; 32]],
    reviewers: &[[u8; 32]],
) -> Result<crate::tools::VerifyReport, String> {
    let m = fetcher.read_manifest().await.map_err(|e| format!("manifest: {e}"))?;
    let mut r = crate::tools::VerifyReport::default();
    for (i, e) in m.entries().iter().enumerate() {
        r.checked += 1;
        let who = format!("line {} ({}/{}, {})", i + 1, e.provides.namespace, e.provides.name, &e.artifact.to_hex()[..12]);
        if !trusted.is_empty() && !e.verify_provenance(trusted) {
            r.problems.push(format!("{who}: provenance does not verify against any trusted publisher"));
        }
        if let Some(p) = crate::tools::acceptance_problem(e, reviewers) {
            r.problems.push(format!("{who}: {p}"));
        }
        match fetcher.hash_matches(&e.artifact, crate::http_source::DEFAULT_RANGE_CHUNK_BYTES).await? {
            None => r.problems.push(format!("{who}: bytes are not in the store")),
            Some(false) => r.problems.push(format!("{who}: bytes in the store do not hash to their address")),
            Some(true) => {}
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_source::DiskStagedSource;
    use crate::tools::signing_key_from_hex;

    fn scratch(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "mycelium-objstore-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn file_url(dir: &std::path::Path) -> String {
        format!("file://{}", dir.display())
    }

    /// The store URLs from the environment — `MYCELIUM_S3_TEST_URL=s3://bucket/prefix` (S2; CI
    /// runs an S3-compatible store, Adobe's S3Mock) and `MYCELIUM_GCS_TEST_URL=gs://bucket/prefix`
    /// (S3; CI runs `fake-gcs-server`, reached through `GOOGLE_BASE_URL` with a static
    /// `GOOGLE_BEARER_TOKEN` the emulator ignores — the put path fetches a credential regardless of
    /// `skip_signature` in `object_store` 0.14), each with the builders' credentials beside it. With neither
    /// set, the local `file://` store stands in and the test says so: the adapter is exercised, the
    /// cloud is not. An emulator proves the code path; only a real bucket proves the cloud (S4).
    fn test_store_urls(dir: &std::path::Path) -> Vec<(String, bool)> {
        let mut urls = Vec::new();
        for var in ["MYCELIUM_S3_TEST_URL", "MYCELIUM_GCS_TEST_URL"] {
            if let Ok(u) = std::env::var(var)
                && !u.is_empty()
            {
                urls.push((u, true));
            }
        }
        if urls.is_empty() {
            eprintln!("MYCELIUM_S3_TEST_URL and MYCELIUM_GCS_TEST_URL unset — exercising the adapter over file://, not a bucket");
            urls.push((file_url(dir), false));
        }
        urls
    }

    const DESCRIPTION: &str = "kind = \"blob\"\nbytes = \"weights.bin\"\n[provides]\nns = \"llm\"\nname = \"weights\"\n[requires]\ndisk_bytes = 3145728\nmem_bytes = 0\n";

    /// S2's exit gate, and S3's over a second store: publish through the adapter, read the
    /// manifest back from the store, stage the blob by ranged pull to disk, verify provenance and
    /// the streamed hash — once per configured store URL.
    #[tokio::test]
    async fn publish_stage_and_verify_through_the_store() {
        for (i, (url, is_bucket)) in test_store_urls(&scratch("s2-urls").join("store")).into_iter().enumerate() {
            publish_stage_and_verify_over(&url, is_bucket, &format!("s2-{i}")).await;
        }
    }

    async fn publish_stage_and_verify_over(url: &str, is_bucket: bool, tag: &str) {
        let dir = scratch(tag);
        let url = if url.starts_with("file://") { file_url(&dir.join("store")) } else { url.to_string() };
        let url = url.as_str();
        // 3 MiB of generated bytes, larger than the 1 MiB in-memory bound set below.
        let bytes: Vec<u8> = (0..3u64 * 1024 * 1024).map(|i| (i.wrapping_mul(31) ^ (i >> 5)) as u8).collect();
        std::fs::write(dir.join("weights.bin"), &bytes).unwrap();
        std::fs::write(dir.join("weights.toml"), DESCRIPTION).unwrap();
        let key = signing_key_from_hex(&"a5".repeat(32)).unwrap();

        let fetcher = ObjectStoreFetcher::from_url(url, EgressPolicy::default()).unwrap().with_max_bytes(1 << 20);
        let out = publish_to_store(&dir.join("weights.toml"), &fetcher, &key).await.unwrap();
        assert_eq!(out.artifact, ArtifactId::of(&bytes));

        // The manifest is in the store, and a second fetcher over the same URL reads it.
        let again = ObjectStoreFetcher::from_url(url, EgressPolicy::default()).unwrap();
        let m = again.read_manifest().await.unwrap();
        assert_eq!(m.entries().len(), 1);
        assert!(list_store(&again).await.unwrap().contains("llm"));

        // Whole-object is refused past the bound by name; ranged staging works.
        let err = fetcher.fetch_remote(&out.artifact).await.unwrap_err();
        assert!(err.contains("past the in-memory bound"), "{err}");
        let stage = DiskStagedSource::open(Arc::new(fetcher), dir.join("stage")).unwrap().with_chunk_bytes(1 << 20);
        assert!(stage.stage_artifact(&out.artifact).await, "the blob stages by ranged pull");
        assert_eq!(std::fs::read(dir.join("stage").join(out.artifact.to_hex())).unwrap(), bytes);

        // verify: provenance and a streamed hash, over the store.
        let r = verify_store(&again, &[key.verifying_key().to_bytes()], &[]).await.unwrap();
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        let other = signing_key_from_hex(&"5a".repeat(32)).unwrap().verifying_key().to_bytes();
        let r = verify_store(&again, &[other], &[]).await.unwrap();
        assert_eq!(r.problems.len(), 1, "{:?}", r.problems);

        // A miss is a miss.
        assert_eq!(again.size(&ArtifactId::of(b"nobody")).await.unwrap(), None);
        assert_eq!(again.fetch_remote(&ArtifactId::of(b"nobody")).await.unwrap(), None);

        if is_bucket {
            eprintln!("the store adapter exercised against {url}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_egress_gate_refuses_a_store_host_before_any_client_is_built() {
        let gated = EgressPolicy { allow_hosts: vec!["library.allowed.example".into()] };
        let err = ObjectStoreFetcher::from_url("s3://some-bucket/prefix", gated).unwrap_err();
        assert!(err.contains("egress policy denies"), "{err}");
    }
}
