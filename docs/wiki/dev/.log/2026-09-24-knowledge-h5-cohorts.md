## [2026-09-24] ingest | knowledge cohorts, declared at admission (Boundary H, M2 / H5)

Up: [dev](../dev.md) · record `docs/design/knowledge-cohorts.md` (new ADR, adopted) · code `src/knowledge/cohort.rs`,
`src/knowledge/resolution.rs` · plan `docs/plans/boundary-h.md` (PR #379).

**Finding.** `ReaderPolicy::group_of` resolved by first match, so overlapping groups made independence
order-dependent. Nothing let an operator declare a fleet. While building the union-find, a test caught a real design
bug: taking edges only from supporters left a silent shared member out of the graph, so two cohorts joined only
through that member did not merge. The graph now includes every member of every configured group and every
in-force declaration.

**Change.**
- `CohortDeclaration`, signed by a trusted operator through P1's external path, and `CohortView` for offers and
  placements.
- Grouping as connected components.
- Sticky dependence: expiry marks a declaration stale, and only supersession removes a member.
- Historical grouping.
- `UndeclaredRule` and `StaleRule`, with exclusions `Undeclared` and `StaleCohortOnly`.
- Five resolution tests (including the reviewer's exact expiry and partition case) and four cohort tests.

**Kept honest.** Control, not lineage. `CohortView` is not durable. Challenges get the same components in H1.
Stacked on K2 (#389) for `verify_signed_by`.
