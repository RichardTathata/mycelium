## [2026-09-24] ingest | challenge admission (Boundary H, M2 / H1)

Up: [dev](../dev.md) · record `docs/design/knowledge-cohorts.md` §4 (H1 adopted) · code `src/knowledge/resolution.rs`
· plan `docs/plans/boundary-h.md` (PR #379).

**Finding.** Any single challenger decided a verdict, with no threshold and no grouping, which is Boundary H's jamming
gain. Rejection reasons were one per record and unbounded.

**Change.**
- `min_challenge_groups`, counted by H5's components. The default of 1 is unchanged behaviour.
- Two routes past the threshold, kept distinct: `DisownedByProvider` (mechanically verified: the provider's own
  current challenge of its release) and `DecisiveSource` (policy).
- `ChallengeReport`, including `evidenced_unadmitted`.
- `max_examined` gives `budget_exhausted` and `InsufficientEvidence`; `max_reported` caps detail. Reasons are one
  per challenger.
- Seven tests. A 100,002-record storm resolves in about 2.9 s in a debug build (measured).

**Kept honest.** P2's entitled-authority revocation is not yet a route. Per-issuer storage and ingestion caps moved to
K3c: they belong to the store, which K3a restructured on a sibling branch. The currency index still scans the whole
store.
