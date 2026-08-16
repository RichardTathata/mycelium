# ingest — the fresh-pass closing assessment + the ingest HTTP edge (2026-08-16)

**The workstream's closing check** — a from-artifacts re-derivation (the FTT wiki article, the
Mycelium code, the Transparency_Platfrom repo), deliberately not leaning on the session's
accumulated view. Verdict: **Mycelium is optimal for the council-wiki exactly where coordination,
not computation, is the bottleneck** — concurrent agent writers, distributed per-council
single-writer ingest, and role failover, over their git truth — adopted incrementally at named
triggers (agent-session serialization hurts · estate regen wall-clock hurts · the scraper fleet
wants shared workers). Their four hard guarantees (quote verification, blocking validation, git
audit, byte-identical round-trip) survive only because git stays truth — the inversion reconfirmed.

**Three findings the fresh pass added:**
- **A (gap, now CLOSED):** the bulk-ingest path was Rust-RPC only while FTT's pipeline is Python.
  Shipped: `POST /gateway/wiki/ingest` (reference + optional `timeout_secs` → `IngestSummary`;
  the payload rule holds at the HTTP edge — the reference travels, never the batch) + `ingest`
  verbs on both SDKs (`mycelium-py` `Wiki.ingest`, `mycelium-ts` `Wiki.ingest`). Gate:
  `gateway_ingest_submits_a_reference_and_returns_the_summary` (applies via HTTP against a curator
  with an `FsBatchSource`; a bad reference surfaces as 502, not a hang). tsc + py-compile clean.
- **B:** FTT **already chose Mycelium** for the scraper run-fleet (their scraper-v3 plan,
  decision 12, 2026-07-16: councils drained from the tuple space by N live-tunable pull workers
  via `mycelium-py`) — the substrate has a second consumer in the same repo, so the wiki adoption
  stacks on a planned dependency rather than introducing one.
- **C:** the incremental-adoption property is itself an optimality argument: with git as truth,
  every existing FTT workflow (hand edits + hooks, the in-repo reviewer, the pipeline writing
  directly) keeps working during migration — adoption is per-council and reversible.

**Remaining, all FTT-side:** `CouncilWikiFormat` + their conformance gate · `S3BatchSource` · the
real validator as the batch-gate command · the councillor-GDPR DPIA line · council-scale runs
(supersede the ten-council numbers; double as Paper 1's production case study).
