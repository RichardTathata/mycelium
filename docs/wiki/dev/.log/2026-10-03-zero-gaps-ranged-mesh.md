## [2026-10-03] ingest | zero gaps Z3 — a blob past the frame cap crosses the mesh in ranges

**What:** `artifact.size` + `artifact.fetch_range` (`MESH_RANGE_CHUNK_BYTES` = 4 MiB) answered by
`serve_artifacts` beside `artifact.fetch`; `MeshRangedFetcher: RangedBlobFetcher`; the stem's mesh
path is `DiskStagedSource` over it, staged under `<placement_root>/stage`; `StemOptions.stage_dir`,
`mycelium-stem --stage-dir`. Plan D3. The bulk transport was the wrong tool: its reply rides a
frame-bound signal and a pull is the puller's choice of ranges.

**Durable knowledge:** the frame cap is a property of one reply, not of the mesh — a pull that
chooses its ranges never meets it, and the holder needs no new state (a ranged source serves from
disk; a whole-object source serves a slice). One staged source object must serve both the
provisioner and the ticker, or the runtime reads a stage the ticker never filled — the same rule
the in-memory cache had.
