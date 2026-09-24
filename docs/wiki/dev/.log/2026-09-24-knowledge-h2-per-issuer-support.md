## [2026-09-24] ingest | knowledge resolution counts support per issuer (Boundary H, M1 / H2)

Up: [dev](../dev.md) · record `docs/design/knowledge-layer.md` §4 · code `src/knowledge/resolution.rs` · plan
`docs/plans/boundary-h.md` (PR #379, proposed) milestone M1.

**Finding.** `classify` collected supporting issuers into a `Vec`, one entry per supporting record, and compared its
length with `min_supporting`. One issuer filing five supporting assessments therefore met `min_supporting = 2` by
itself. Independence was deduplicated by control group and was unaffected, which is why the default policy (one
independent group) hid the gap: it only showed when `min_supporting` exceeded `min_independent`.

**Change.**
- Supporting issuers are a `BTreeSet`, so one issuer is one supporter.
- Every `supporting` count a verdict reports now counts issuers.
- `Verdict` and `RejectionReason` are `#[non_exhaustive]` ahead of H1, and their `_` arms must fail safe.
- Two new tests: `one_issuer_repeating_itself_is_one_supporter` and
  `reported_support_counts_issuers_not_records`.

**Kept honest.** Challenges are still counted per record; that is H1's job, and the doc comment on
`Conflicted.challenging` says so. The threat model's H2 entry is in the unmerged PR #379, so marking it *in force*
waits for that merge.
