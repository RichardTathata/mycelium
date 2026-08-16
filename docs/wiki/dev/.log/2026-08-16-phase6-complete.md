# ingest — Phase 6 complete (all Mycelium-side items) (2026-08-16)

**P6.6 shipped** (the last item): the ingest responder's apply and the drain-round publish now run
on the blocking pool (`spawn_blocking` — sync git/network I/O off the tokio workers);
`submit_batch_with_timeout(reference, timeout)` added with the **sizing contract documented on both
ends: a batch = one meeting** (the boundary commit, the batch-atomic gate, and the 60s default are
sized to that unit); `councils/councils.md` stays FTT-owned (regenerable index).

**Phase 6 close-out.** Every Mycelium-side gap from the 2026-08-15 step-back critique is closed
with a gated build, and both measurements the plan demanded are recorded:
- P6.1 batch commits + batch gate (whole-meetings-only restored; ~5× commit-volume cut)
- P6.2 read plane — **330 ms / 600 pages** measured (≈3 s extrapolated over Edinburgh)
- P6.3 failover — the litmus restored; the un-pushed-tail residual tested, not hidden
- P6.4 contention — **5.5 / 3.0 batches/s** (shared/deployed) at zero spurious failures; the gate
  found four real defects (init race · cross-instance temp-index collision · merge-tree falsified
  → subtree splice · backoff starvation)
- P6.5 the pluggable `PageFormat` codec, proven end-to-end with a custom format
- P6.6 lower tier

**What remains is FTT-side by design** (their tracker): `CouncilWikiFormat` + their conformance
gate, `S3BatchSource`, the real Node validator as the batch-gate command, the councillor-GDPR DPIA
line, and council-scale runs — whose numbers supersede the ten-council measurement (the 391
extrapolation stays deliberately unclaimed). The scale run doubles as Paper 1's production
work-distribution case study.

Nine CI-green pushes across the substrate arc (five phases + critique/plan + P6.1–P6.6 batches),
each watched to green before the next stacked.
