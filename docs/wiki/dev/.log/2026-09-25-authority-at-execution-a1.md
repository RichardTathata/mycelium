## [2026-09-25] ingest | authority at execution (Boundary H, item A1)

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/design/authority-at-execution.md` (ADR,
adopted) · code `src/mandate/authority.rs`, `src/agent/action_evaluator.rs` (`allowances_without_mandate`).

**Finding.** Expiry stopped new admissions and nothing else: admitted, queued, retried and delegated work ran on, and
a missing revocation read the same as "none". `ResourceAuthority::check` already existed at the resource, so A1
composes on it rather than adding a second authority check.

**Change.**
- `ExecutionGate`: strict profile only; AE2 tier declared; advisory refused.
- `AuthorizedWork`, with clamped windows and delegation that never extends them.
- Authority-signed `RevocationCheckpoint`s with replay protection, and the exact freshness predicate.
- `StopContract::drain_bound`, and `drain_report` for T_admit and T_drain.
- A policy-hole check on the reference evaluator.
- Sixteen tests plus one, including both clock extremes, and partition or silence denying.

**Kept honest.** No resource is wired to the gate yet. The revocation view is in memory (fail closed on restart). The
Cedar adapter needs its own policy-hole check.
