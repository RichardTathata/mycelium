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

1. **What it is.** Two or three plain sentences, and a link to the concept's entry in the
   [concepts glossary](../guide/00-concepts.md).
2. **When to use it, and when not.** One line each.
3. **Turn it on.** The settings that enable the feature, each linked to its entry in
   [`configuration.md`](../reference/configuration.md) or [`unit-file.md`](../reference/unit-file.md), and a link
   to a configuration or unit file that a test loads. For a feature enabled in code, the section opens with the
   fixed sentence "This feature is turned on in code", then lists one item per line: the item's name in
   backticks, linked to the file that defines it. It ends with the test that runs that code, its name in backticks
   and in the `test-coverage` universe. A test that reads the page is preferred; a test that runs a hand-kept copy
   is recorded as such.
4. **See it work.** A link to the feature's row in the [example index](../../examples/README.md), which holds
   the run command, the prerequisites and the CI status. If there is none, the section opens with the fixed
   sentence "No runnable example exists" and links the test that runs the feature end to end; the feature
   registry records `example: none`.
5. **What it guarantees, and what it does not.** The guarantee and rule ids, each linked to where it is
   catalogued, the plan rows whose *not built* items apply, and the
   [what-is-proven](../operations/what-is-proven.md) ledger.
6. **Go deeper.** The guide chapter, the operations runbook, the design record and the code.

## Link, do not copy

A page links to its sources and copies nothing from them: no paragraph from a guide, no snippet of
configuration, no evidence status. A copy drifts from its source and nothing notices; a link either resolves
or fails a check. Where a page must name a limit, it names it in a few words and links to the place that
states it.

## The pages

| Page | Feature | Capability-map row |
|---|---|---|
| [Activation egress proxy](activation-egress-proxy.md) | An activation command reaches hosts by name through a proxy its stem runs, with credentials attached at the proxy | Acquire and adapt capabilities |
| [Fleet-wide history limits](fleet-history-limits.md) | A history-limited rule counts an actor's calls across the fleet, exactly by consensus seats or approximately by a gossiped tally | Constrain actions |
| [Inference routing as policy](inference-routing.md) | The action evaluator decides each routed model call on its destination, locality and data class | Constrain actions; Run tools and reasoning |
