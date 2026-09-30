# 2026-09-30 — S3: the GCS fixture

**What:** the object-store adapter's test (`mycelium-wasm-host/src/object_store_source.rs`) runs its
sequence once per configured store URL — `MYCELIUM_S3_TEST_URL` and now `MYCELIUM_GCS_TEST_URL` — and
CI adds `fake-gcs-server` beside S3Mock, reached through `GOOGLE_BASE_URL` + a static `GOOGLE_BEARER_TOKEN`.

**Durable knowledge:**
- **`object_store` 0.14 has no emulator-host variable for GCS** (Azure has one); the way in is the
  builder's `google_base_url` config key, which `parse_url_opts` reads from the environment like every
  other builder option. **`google_skip_signature` is not enough**: the first CI run showed the put
  path (`Request::send` in `gcp/client.rs`) fetching a credential regardless, and with none configured
  it asked the GCE metadata server. A static `google_bearer_token` is a credential that needs no
  server, and the emulator ignores it — so CI sets two variables and the code is unchanged.
- **One feature, two builders** (D11): the plan's S3 row asked for `store-gcp` without `store-aws` to
  compile; S2 decided one `object_store` feature with both, so the gate is the fixture, not a split.
- **An emulator proves the code path, not the cloud.** Said in the runbook; S4 stays open until a real
  bucket is run and dated.

**Pages touched:** plan row S3, CHANGELOG, `operations/artifacts.md`, `ci.yml`.
