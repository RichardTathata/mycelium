## [2026-10-20] ingest | Feature pages F2: the registry's invariant made to hold (#655 review)

The independent review of #655 found that the gate's invariant — every feature has an id the registry must map —
did not hold: only 31 of the glossary's quick-reference rows carried an `<a id>` (federation, receipts, MCP tools,
the mailbox, the contract net and AgentFacts had none), crates without `[features]` contributed nothing, and the
unit-file source read the docs' headings rather than the code. Fixed at the root:

- every quick-reference row must carry an `<a id>` (35 anchors added to `docs/guide/00-concepts.md`, stable slugs);
- a seventh source, `crate:<name>`, for every workspace crate;
- the unit-file keys read from `NodeCapabilityConfig` (`rename` honoured) and its confine/egress tables, checked
  against the headings both ways;
- implicit Cargo features (an optional dependency no feature names as `dep:`) counted;
- a companion guarantee with a non-literal id or an aliased descriptor refused, and test modules skipped by a brace
  match that skips strings, chars and comments;
- registry keys outside the known set refused (a `link =` typo had dropped a check silently); one page, one
  feature; no subdirectories;
- reference-style, titled and `<a href>` links refused on feature pages;
- a backticked snake_case name in *See it work* is a test and must link its file; in CI a named test must have run
  in its file's target (an integration file's binary, a `src/` file's crate), not merely under its name.

The suite has 57 cases: 54 plants and three baselines, one of them a planted page with a real example so the
example branch runs. Each plant for a new check was run against the first-round gate and not caught. With each of
the seven branches the review found unplanted disabled in turn, the suite fails at that branch's plant. Registry:
47 → 58 features (55 of them todo), 10 exclusions. Prose on pages stays with the F-S `/doc-coverage` pass. The stem-side Rust test the review
suggested was not added: with non-literal ids refused, it would read the same literal the gate reads.

Pages: `dev/examples.md` (seven sources). Repo: `docs/guide/00-concepts.md` (anchors),
`docs/features/registry.toml`, `docs/features/README.md`, `docs/plans/feature-pages.md` (§3, §4.2),
`scripts/check-feature-pages.py`, `scripts/test-check-feature-pages.py`.
