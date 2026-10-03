## [2026-10-03] ingest | zero gaps Z1 — a stem reads an object store

**What:** `StemSource::Store { url }` over `ObjectStoreFetcher` + `DiskStagedSource` (feature
`object_store`), `--library <url>`, `--manifest-source <url>`, a store-backed librarian that mirrors the
manifest's blobs to its stage, the `catalog_store` suite profile against S3Mock, and
`a_store_backed_stem_installs_from_the_bucket` under `object_store,stem` in the `wasm-host` job. Plan D1.

**Durable knowledge:** once the mesh path stages to disk (Z3), a store is just another
`RangedBlobFetcher` behind the same stage — the stem's provisioner never knew the difference, and the
re-serve to peers came for free. The librarian over a store needed one thing the file librarian did
not: a mirror loop, because `serve_artifacts` serves what the stage holds and a manifest read from a
store stages nothing by itself.
