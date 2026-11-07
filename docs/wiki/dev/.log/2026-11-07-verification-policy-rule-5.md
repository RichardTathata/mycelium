# 2026-11-07 — verification policy rule 5

## [2026-11-07] ingest | the fifth verification-policy rule: a feature's page changes with the feature

Row F4 of [`plans/feature-pages.md`](../../../plans/feature-pages.md), its first half (F-P and F-S's skill change);
the rule agreed with the maintainer 2026-10-19. A PR that adds or changes a feature adds or updates its page under
`docs/features/` and its entry in `registry.toml`, and the independent reviewer checks both.

- [`dev/testing/verification-policy.md`](../testing/verification-policy.md): *The five rules*, rule 5 and where it
  is enforced (the gate, `/adversarial-review` check 7, the `/doc-coverage` pass).
- `CLAUDE.md` § Verification policy, `CONTRIBUTING.md`, [`AGENTS.md`](../../AGENTS.md)'s routing row,
  [`dev/testing/testing.md`](../testing/testing.md)'s pointer, and the PR template's field 5.
- `.claude/commands/adversarial-review.md` check 7; `.claude/commands/doc-coverage.md` Step 4b, the feature-pages
  pass (its four questions). The pass itself is not yet run: it runs once F3's pages land, and F4 closes then.
- Left as written, being dated records of the four-rule policy: the v2.26.0 release entries in `CLAUDE.md`,
  `ROADMAP.md` and `wiki.md`, `CHANGELOG.md`, and the 2026-10-06 log.
