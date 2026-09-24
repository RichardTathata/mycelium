## [2026-09-24] ingest | present eligibility at resolution (Boundary H, M2 / K1b)

Up: [dev](../dev.md) · record `docs/design/knowledge-validity.md` §2 (K1b adopted) · code
`src/knowledge/resolution.rs`, `src/knowledge/correction.rs` · plan `docs/plans/boundary-h.md` (PR #379).

**Finding.**
- `classify` never called `correction::standing`, so a retracted assessment kept supporting a release. The
  retraction machinery existed and resolution did not use it.
- `DependencyIndex::build` honoured a `Retracts` link from any record in the store, including unverified ones.
  Once unchecked records are excluded, that is a suppression path.

**Change.**
- `classify_eligible` returns `Classification { verdict, excluded }`. It re-verifies retained signatures against
  the current key view, and its `Exclusion` covers authenticity, present authority and currency.
- `classify` shares the core: it narrows only, and does not re-verify.
- `UncheckedRule` defaults to `Count` for compatibility; the confined profile requires `Exclude`.
- Supersession counts only when same-issuer.
- `DependencyIndex::build_filtered` limits withdrawal to authentic records.
- Seven tests (`resolution::tests::k1b`). The example's narrative is unchanged.

**Kept honest.** The plan's "current policy revision" layer is not built (`ReaderPolicy` has no revision). The
`Count` default is a compatibility choice, recorded for the §6.6 ledger.
