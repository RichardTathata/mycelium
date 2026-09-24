## [2026-09-24] ingest | Boundary H plan rev 0.4: adoption corrections

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/plans/boundary-h.md` (rev 0.4).

**Finding.** Round 3 of the external review asked for three corrections to accompany adoption:
- H1's closing sentence contradicted its own `evidenced_unadmitted` rule;
- `RunToCompletion` had no T_drain bound of its own;
- the revocation clock rule was not executable.

Separately, implementing P1 found that H4's proposed checkpoint record already exists (`AuditCheckpoint`,
`sys/audit-checkpoint/`).

**Change.**
- H1 uses the reviewer's wording.
- `RunToCompletion` has its own T_drain bound (*s* + remaining permitted duration + confirmation), requires a
  resource-enforced `max_duration`, and reports classes without a demonstrable bound as `Unbounded`.
- A1 defines *s* as each clock's deviation from real time, gives the exact freshness predicate with its safety and
  liveness bounds, requires *F* > 4*s* and *I* + *D* ≤ *F* − 4*s* at start-up, and tests both clock extremes.
- H4 reuses `AuditCheckpoint` and adds only retention and comparison.
