## [2026-09-30] ingest | S3: the GCS fixture, and why the emulator half did not close

**What:** the object-store adapter's test (`mycelium-wasm-host/src/object_store_source.rs`) runs its
sequence once per configured store URL — `MYCELIUM_S3_TEST_URL` and now `MYCELIUM_GCS_TEST_URL` — so a
GCS run needs only a URL and credentials. Two CI runs tried `fake-gcs-server`; the emulator cannot
take the crate's put, and the row is ◐ with the GCS gate moved to S4's real bucket.

**Durable knowledge:**
- **`object_store` 0.14 has no emulator-host variable for GCS** (Azure has one); the way in is the
  builder's `google_base_url` config key, which `parse_url_opts` reads from the environment like every
  other builder option. **`google_skip_signature` is not enough**: the first CI run showed the put
  path (`Request::send` in `gcp/client.rs`) fetching a credential regardless, and with none configured
  it asked the GCE metadata server. A static `google_bearer_token` is a credential that needs no server.
- **`fake-gcs-server` cannot take the crate's put.** Its router (`UseEncodedPath`, route
  `/{bucket}/{object:.+}` → `insertObject`) dispatches on `uploadType`, and with none it accepts only a
  signed-URL upload (`X-Goog-Algorithm` in the query); `object_store` puts with a plain XML-API PUT
  under a bearer — `invalid uploadType`, the second CI run. Reading the emulator's `upload.go` settled
  it; no emulator with a bearer-authenticated XML PUT is known. The emulator step is out of CI (the
  job must not be red for a fixture the code cannot use), the test loop stays, and the GCS gate is a
  real bucket.
- **One feature, two builders** (D11): the plan's S3 row asked for `store-gcp` without `store-aws` to
  compile; S2 decided one `object_store` feature with both, so the gate is the fixture, not a split.
- **An emulator proves the code path, not the cloud.** Said in the runbook; S4 stays open until a real
  bucket is run and dated.

**Pages touched:** plan row S3, CHANGELOG, `operations/artifacts.md`, `ci.yml`.
