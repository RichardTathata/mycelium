## [2026-09-28] ingest | S2: the object-store adapter

**What:** `mycelium-wasm-host/src/object_store_source.rs` (`ObjectStoreFetcher`, `publish_to_store`,
`list_store`, `verify_store`) behind feature `object_store` (the `object_store` crate 0.14, `aws` + `gcp`);
`ManifestSource` on the librarian; `Manifest::upsert`; the artifact tool takes a store URL.

**Durable knowledge:**
- **One crate, one feature.** The plan's `store-aws`/`store-gcp` split became a single `object_store`
  feature carrying both builders: the crate's URL parser (`parse_url_opts`) picks the backend from the
  scheme and reads every builder option from the environment, so the split bought nothing.
- **Credentials are the environment's** (D13): `parse_url_opts(url, std::env::vars())`. For an S3-compatible store:
  `AWS_ENDPOINT`, `AWS_ALLOW_HTTP=true`, the two keys, a region. The egress policy is checked on the URL
  *before* a client is built, and again on every call.
- **`object_store`'s trait methods return `impl Future`** (no `async_trait`); `Box<dyn ObjectStore>`
  works because the crate implements the trait for its own boxed/`Arc` forms.
- **`LibrarianConfig` gained a field** — every literal in the tree needed `manifest_source: None`
  (nine: two librarian tests, the stem test, the stem binary, five coop demos). An additive field is
  still a break for an exhaustive literal; the changelog says so.
- The S3 test is **environment-selected**: `MYCELIUM_S3_TEST_URL` points it at a bucket (CI: Adobe's `s3mock` container with a bucket from its environment — MinIO's images failed to pull from Docker Hub and quay.io, three runs in a row, a supply-chain fact worth knowing); unset, it runs over `file://` and prints that it did —
  the adapter is exercised either way, the cloud only in CI. No real bucket has been used (S4).

**Pages touched:** `artifacts.md` § Remote blob stores; `what-is-proven.md`; the plan's D14, S2, S3;
`CHANGELOG.md`.
