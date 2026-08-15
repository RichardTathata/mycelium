# ingest — council-substrate Phase 4: the bulk-ingest claim-check (2026-08-15)

**Shipped:** the claim-check path. `IngestBatch`/`IngestPage` (staged payload format, serde JSON);
`BatchSource` trait (`FsBatchSource` ref impl — an `S3BatchSource` is the deployment impl of the
same trait, exactly the FsStore/S3Store pattern); `apply_batch` — the deterministic serial write
phase factored **pure** (each page via `write_page` in batch order, through the Phase-3 gate:
refusals recorded per page, the rest of the meeting never lost; other store errors abort a
resubmittable batch); curator surface `wiki.{group}.ingest` (membership-gated RPC mirroring the
access broker, served only when `CuratorBrain::with_batch_source` is configured) + worker-side
`Wiki::submit_batch(reference)` with a curator-local fast path.

**The payload rule made structural:** the RPC carries the *reference string*; the leaves ride the
bulk store (S3/FS), never gossiped KV or the mesh.

**Two properties worth remembering:**
1. **Byte-identical determinism** — `apply_batch` vs a serial in-process writer produce identical
   `HEAD^{tree}` AND identical commit counts (the Phase-4 gate proves it, not asserts it).
2. **Idempotent resubmission for free** — `write_page` is a full replace and an unchanged rewrite
   records nothing, so re-submitting after a partial failure no-ops the already-applied pages.
   Recovery = resubmit, no bookkeeping.

**Gates (green):** byte-identical + resubmit-noop; per-page gate refusals (Phase 3 × 4); the
remote leg over a real two-node mesh (worker submits reference, curator fetches from its own
source, applies, worker reads committed truth through its own store handle — E4).

Remaining: Phase 5 (work distribution — tuple-space council leases + kill-a-worker redistribution;
Paper 1's production case study).
