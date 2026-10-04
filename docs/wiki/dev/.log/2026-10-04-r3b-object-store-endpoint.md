## [2026-10-04] ingest | realignment repairs R3b — the object-store gate is on the endpoint

**What:** `mycelium-wasm-host/src/object_store_source.rs` (`dial_hosts`, re-exported; `from_url` and
every request gate on the derived endpoints), two tests, the changelog, the plan row, and this log.

**Durable knowledge:**

- **A store URL's host is not always a host.** For `s3://bucket/prefix` and `gs://bucket/prefix`
  the URL's host is the bucket name; the client dials a regional or configured endpoint. Gating the
  URL gated the bucket name, so a bucket named like an allowed host opened a store on an endpoint
  the list never named. Gate on what `object_store` will dial, derived from the same option keys it
  reads (`parse_url_opts` lower-cases them), and refuse what cannot be derived.
- **The derivation must follow the builder's precedence.** `AWS_ENDPOINT_URL_S3` beats
  `AWS_ENDPOINT_URL` / `AWS_ENDPOINT`; for an `http(s)://` store the builder applies endpoint options
  *after* the URL, so both hosts are gated. If `object_store` changes its keys, `dial_hosts` and its
  test are where it shows.
- **Outside the gate, stated:** the cloud identity's credential traffic (instance metadata, STS) and
  redirects inside `object_store`'s own HTTP client, which this adapter does not build.
