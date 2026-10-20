# Feature pages

One short page per feature: what it is, how to turn it on, and where to see it work. A feature page is an
entry point, not a manual. It gathers links to the reference, the guide, the example index, the catalogues
and the code, so a reader can find everything about one feature from one place. The
[capability map](../capabilities.md) is meant to be the index; today its rows link the pages that exist, and every
row lists its pages once the remaining pages are written (the plan's row F3).

The plan that introduced these pages is [`plans/feature-pages.md`](../plans/feature-pages.md).

## The six sections

Every page has these six sections, in this order, with these headings. Copy
[`_template.md`](_template.md) to start one.

1. **What it is.** Two or three plain sentences, and links to the concept's entries (one or more) in the
   [concepts glossary](../guide/00-concepts.md).
2. **When to use it, and when not.** One line each.
3. **Turn it on.** The settings that enable the feature, each linked to its entry in
   [`configuration.md`](../reference/configuration.md) or [`unit-file.md`](../reference/unit-file.md), and a link
   to a configuration or unit file that a test loads. For a feature enabled in code, the section opens with the
   fixed sentence "This feature is turned on in code", then lists one item per line: the item's name in
   backticks, linked to the file that defines it, whose last path segment is a `fn`, `struct`, `enum`, `trait` or
   `const` in the linked file. A configuration field the feature reads is its own line, `name` in backticks linked
   to [`configuration.md#the-fields`](../reference/configuration.md#the-fields). The section ends with the test that
   runs that code: its name in backticks, a `fn <name>` in the linked file (checked pre-push), listed by CI's test
   universe and matched by no entry of `scripts/test-coverage-exceptions.txt` (checked in CI). A test that reads
   the page is preferred; a test that runs a hand-kept copy is recorded as such.
   A guide snippet that no test runs is recorded as such, and the gap goes to the `/doc-coverage` pass.
4. **See it work.** A link to the feature's row in the [example index](../../examples/README.md), which holds
   the run command, the prerequisites and the CI status. If there is none, the section opens with the fixed
   sentence "No runnable example exists" and links the test that runs the feature end to end: its name in
   backticks, a `fn <name>` in the linked file, listed by CI's test universe and matched by no entry of
   `scripts/test-coverage-exceptions.txt`. The feature registry records `example: none`, and the gate fails if the
   example index later gains a row for that feature.
5. **What it guarantees, and what it does not.** The guarantee and rule ids, each linked to where it is
   catalogued, the plan rows whose *not built* items apply, and the
   [what-is-proven](../operations/what-is-proven.md) ledger. A guarantee id a companion registers (such as
   `stem.*`) is not in the guarantee catalogue: link it to where its states are described. Until the catalogues, the
   ledger, the configuration table and the plans have per-entry anchors, link the table and name the row beside the
   link.
6. **Go deeper.** The guide chapter, the operations runbook, the design record and the code. Where a slot has
   nothing, say so: "no design record; the plan row is the record", or no runbook, naming what covers the ground
   instead.

## Link, do not copy

A page links to its sources and copies nothing from them: no paragraph from a guide, no snippet of
configuration, no evidence status. A copy drifts from its source and nothing notices; a link either resolves
or fails a check. Where a page must name a limit, it names it in a few words and links to the place that
states it.

## The registry and the gate

[`registry.toml`](registry.toml) maps every id that names a feature to its page. The ids come from six
sources: the [concepts glossary](../guide/00-concepts.md)'s `<a id>` entries, the
[guarantee catalogue](../reference/guarantee-catalogue.md), the [rule catalogue](../reference/rule-catalogue.md),
the [unit-file](../reference/unit-file.md) sections, the Cargo features of every workspace crate, and the guarantees
a companion registers in code (such as `stem.*`). Each id maps exactly once: to a feature with a page, to a feature
whose page is not written yet (`page = "todo"`, the plan's row F3), or to an exclusion that says why it is not a
reader-facing feature. A feature's page must link every id the registry gives it.

`scripts/check-feature-pages.py` is the gate. It runs in `make check` (through `scripts/check-positioning.sh`) and
in CI, and fails on an id the registry does not map, a registry id no source has, a page the registry does not name,
and on any page whose sections, links, anchors, code items, configuration fields, named tests or example do not
check out. It recognises the markers above this way:

- **A code item** is a link whose text is one backticked name and whose target is a `.rs` file; its last path
  segment must be a `fn`, `struct`, `enum`, `trait` or `const` there.
- **A configuration field** is a backticked name linked to `configuration.md`; it must have a `| name |` row in the
  field table.
- **A test** is a backticked snake_case name of four or more words, not itself a link, in *Turn it on* or
  *See it work*, in the same paragraph or list item as a link to its `.rs` file. It must be a `#[test]` `fn` there
  and must not be excepted by `scripts/test-coverage-exceptions.txt`; in CI the `test-coverage` job also checks
  that some job of the run executed it.
- **No example:** with `example = "none"`, *See it work* opens with the fixed sentence, and the gate fails if the
  example index's capability matrix gains a row whose name contains the page's name in snake case or an
  `example_watch` entry, or that links the page.

Adding or renaming a page means adding or changing its `[[feature]]` entry in the same change. The gate's mutation
suite is `scripts/test-check-feature-pages.py`.

## The pages

| Page | Feature | Capability-map row |
|---|---|---|
| [Activation egress proxy](activation-egress-proxy.md) | An activation command reaches hosts by name through a proxy its stem runs, with credentials attached at the proxy | Acquire and adapt capabilities |
| [Fleet-wide history limits](fleet-history-limits.md) | A history-limited rule counts an actor's calls across the fleet, exactly by consensus seats or approximately by a gossiped tally | Constrain actions |
| [Inference routing as policy](inference-routing.md) | The action evaluator decides each routed model call on its destination, locality and data class | Constrain actions; Run tools and reasoning |
