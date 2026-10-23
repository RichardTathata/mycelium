## [2026-10-23] ingest | Feature pages F2: the confirmation round of #655

The third, confirmation review of #655 confirmed every closure of the first two rounds for the forms they named,
and found the registry's invariant — a source cannot hold a feature the registry does not see — open at two more
edges, each one step from a plant the suite already had:

- the glossary parser read only lines starting `| `; a quick-reference row written `|Term|` or indented one space
  renders as a row and was skipped before the `<a id>` check (the example-index parser had the same shape). Rows
  are now any line whose stripped text starts with `|` (`table_cells`);
- a descriptor built inside an `impl … for GuaranteeDescriptor` block (`Self::new("…"`) or through `new` named as
  a function item escaped the scan; both refused. Comments are blanked before the scan, so a doc comment that
  mentions `GuaranteeDescriptor::new("…"` is neither a registration nor a refusal (it had been read as one).

Also: a group header's italic pointer must carry a `(#…)` link, so a bold term with an italic definition is a term;
a glob workspace member is expanded as cargo expands it (it was a traceback); a `todo` feature's `planned` page may
not be an existing feature's page; `check-positioning.sh` says seven sources. The scan's `src/`-only scope is now
stated (plan §4.2) rather than implied. Suite: 93 cases, 81 plants and 12 baselines; each round-3 plant was run
against the second-round gate first — seven escapes and one crash (recorded on the PR). The confirmation pass on the fix found two more: the comment
blanking had no char-literal token, so a `'"'` opened a pseudo-string and a later `/*` inside a string blanked a
registration (a regression of the fix itself, caught before merge), and an inherent `impl GuaranteeDescriptor` block;
both refused, both planted. The stem-side Rust test
stays not built, with the argument for it recorded: the "same literal" claim was narrowed twice before it held.

**Lesson for the gate family:** a parser that keys on a leading `| ` reads a hand-formatted table; the plant it
needs is the one-character variant of the row it already plants.

Repo: `scripts/check-feature-pages.py`, `scripts/test-check-feature-pages.py`, `scripts/check-positioning.sh`,
`docs/plans/feature-pages.md` (rev 0.6, §4.2).
