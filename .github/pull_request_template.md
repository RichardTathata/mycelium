## What changed and why

<!-- One paragraph. Link the plan row or issue. -->

## Verification policy (`CLAUDE.md` § Verification policy)

**1 · Enumeration.** The invariant this touches, every entry point that reads or writes it, and the grep
that found them. Each is tested, or named here as out of scope with the reason.

```
<grep command(s)>
```

| Entry point | Tested by | Notes |
|---|---|---|
|  |  |  |

**2 · Plan-row evidence** (if this closes or advances a plan row). Each promise of the row, quoted, beside
the code or test that delivers it; *not built* for any it does not.

| Promise (quoted) | Delivered by |
|---|---|
|  |  |

**3 · Tests run by discovery.** New test files are under a directory a CI step runs; none is added to an
explicit file list.

**4 · Adversarial review.** Link to the independent review (`/adversarial-review`) and how each finding was
fixed or answered.

**5 · Feature page.** If this adds or changes a feature: its page under `docs/features/` and its entry in
`docs/features/registry.toml`, added or updated here. Or one of the two exceptions, said here: why the change
leaves the page true; or the entry is at `page = "todo"` and predates F3's close (after F3, a new feature lands
with its page). `check-feature-pages.py` sees glossary concepts, guarantee and rule ids, unit-file keys, Cargo
features, companion guarantees and crates; a configuration field, route or SDK verb it does not see — name the
page that covers it.

## Fail-first

The test that failed before the fix, and its failure output.
