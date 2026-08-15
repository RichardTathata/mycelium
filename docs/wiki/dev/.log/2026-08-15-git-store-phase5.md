# ingest — council-substrate Phase 5: work distribution (ALL FIVE PHASES BUILT) (2026-08-15)

**Shipped:** the work-distribution assembly — deliberately **zero new mechanism**: the tuple space
supplies council work-leases (`worker_timeout_secs`; taken-not-acked re-queues = at-least-once),
Phase-4's ingest supplies the idempotent apply, and the composition is **exactly-once effect** —
the `exactly-once-effect.md` contract holding across TWO companions meeting for the first time.
Test-only dev-dep `mycelium-tuple-space` (production coupling nil: workers and curators meet only
through the store + RPC).

**Gate (green, 32s):** `work_distribution.rs::a_dead_workers_lease_redelivers_and_the_batch_lands_exactly_once`
— the kill is at the WORST point (after submit, before ack), so the redelivered lease re-submits
the batch in full and zero-duplicates is earned by idempotency, not by a convenient death:
duplicate commits = 0, duplicate leaves = 0, lane drained after ack.

**Operational fact worth remembering:** the tuple-space primary's re-queue scan ticks every **30s**
(hardcoded), so redelivery latency ≈ lease + ≤30s, and a parked `take` may not be woken by a
re-queue — **poll, don't park, when waiting on redelivered work** (the gate does).

**The five-phase arc, one day:** GitStore (envelope build) → group-per-council → validator write
gate → bulk-ingest claim-check → work distribution. Every phase's design-record gate is a passing
test; every push was CI-watched to green before the next stacked. What remains is deployment
(FTT-side): S3BatchSource, the real Node validator as validate_cmd, council-scale runs — the scale
run doubles as Paper 1's production case study.
