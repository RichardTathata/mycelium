# Feature pages — one short, checked entry page per feature

**Status:** proposed, rev 0.2. Agreed with the maintainer on 2026-10-19 and not started; work begins after
v2.36.0 ships. No row below is delivered. Rev 0.2 answers the independent review of PR #649.

## 1 · The gap

A reader who wants one feature has no single page that says what the feature is, how to turn it on and
where to see it work. The [capability map](../capabilities.md) routes nine broad needs, such as *Constrain
actions* and *Coordinate work*, to guide chapters, examples and operations pages. It does not route
individual features. It has no column for the setting that enables a feature, and no plain one-paragraph
account of what each feature is.

The pieces exist, but they are scattered. Take the activation egress proxy. It is described in
[`unit-file.md`](../reference/unit-file.md), [guide 9](../guide/09-security.md), the
[crown-jewel runbook](../operations/crown-jewel.md), the concepts glossary, `deprecations.md` §51 and the
rule catalogue. Fleet-wide history limits and inference routing are sections of
[guide 20](../guide/20-authorising-actions.md), with glossary entries. None of the three has one page that
gathers these, and nothing checks that such a page exists or still agrees with the code.

## 2 · The page

Each feature gets one thin page, `docs/features/<feature>.md`, with these six sections in this order. A page
links to its sources and copies nothing from them, because copies are what drift.

1. **What it is.** Two or three plain sentences, and a link to the concept's entry in
   [`00-concepts.md`](../guide/00-concepts.md).
2. **When to use it, and when not.** One line each.
3. **Turn it on.** The settings that enable the feature, linked to their entries in
   [`configuration.md`](../reference/configuration.md) or [`unit-file.md`](../reference/unit-file.md). It
   also links to a configuration or unit file that a test loads. The page shows no snippet of its own.
4. **See it work.** A link to the feature's example in the [example index](../../examples/README.md), which
   keeps the run command and the prerequisites, as the capability map's maintainer note requires.
5. **What it guarantees, and what it does not.** The guarantee and rule ids, each linked to its catalogue,
   plus links to the plan row's *not built* items and the
   [what-is-proven](../operations/what-is-proven.md) ledger. Evidence status stays in the ledger.
6. **Go deeper.** The guide chapter, the operations runbook, the design record and the code.

The capability map becomes the index, and each of its rows lists its feature pages.

## 3 · Keeping the pages true

Three mechanisms keep the pages true, each covering a different kind of check.

| # | Mechanism | Checks |
|---|---|---|
| F-G | **A CI gate**, a sibling script of [`check-materials.py`](../../scripts/check-materials.py) and [`check-front-doors.py`](../../scripts/check-front-doors.py) (which checks the repository's entry pages — the name this plan avoids) that shares their link and anchor resolution | It reads a checked-in **feature registry**: every id from every source maps to a page, or to an exclusion with a stated reason, and the gate fails on any unmapped id. This is the pattern of the reachability registry and the lock-order table, so a new id cannot slip through by being absent from a hand list. **Sources:** the concepts glossary anchors in `00-concepts.md`, the guarantee catalogue, the rule catalogue, the unit-file sections, and the Cargo features of `mycelium`, `mycelium-core`, `mycelium-wasm-host` and the companions. **Many-to-one mapping is expected**, because most guarantee ids are settings of a feature. For example, `tls` maps to `mesh.tls`, `gw.tls` and `id.*`, and `[activation.confine.egress]` maps to `egress.credentials` and `prov.activation_egress`. **Stated exclusions:** build and test features (`cli`, `sim`, `fuzz-internals`, `test-util`, `test-frame-filter`) and declaration vocabulary (`principal`, `[[capability]]`, `[[requirement]]`). **Each page is checked for:** all six sections, links that resolve, settings that exist in the reference, and an example listed in the example index with its matrix status (✓, ✓ᵖ or manual), so the gate does not require a CI run that cannot exist. |
| F-S | **[`/doc-coverage`](../../.claude/commands/doc-coverage.md) gains a feature-pages pass** | It covers the judgement a script cannot make: whether the description is still true of the code, whether the example demonstrates the feature rather than merely running, whether the *not built* links match the plan, and which features merged since the last pass lack a page. It already audits documentation against code, and its rule that "presence is not sufficiency" is the same test. The skill is read before it is changed. `/wiki-lint` gains no feature-pages pass. |
| F-P | **A fifth verification-policy rule**, in [`verification-policy.md`](../wiki/dev/testing/verification-policy.md), in CLAUDE.md's summary of it, and in the `/adversarial-review` checks | A PR that adds or changes a feature adds or updates its page and its registry entry, and the independent reviewer checks both. Drift is then caught at the PR, the same way rule 2 closes plan rows on evidence. Adding a rule to a policy marked non-negotiable is the maintainer's decision, agreed on 2026-10-19. |

## 4 · Delivery

| Row | Work | Exit |
|---|---|---|
| F1 | The template and three pilot pages: the activation egress proxy (rows L and M), fleet-wide history limits (K2) and inference routing (R) | Each pilot page has an independent review against the code, and the template is revised from what the pilots show. |
| F2 | The feature registry and the gate (F-G) | The gate fails on an unmapped id, on a page with a missing section, on a dead link or setting, and on an example absent from the index. Each of these is shown failing with a planted fault. |
| F3 | The remaining pages, an estimated 30 to 40 | The gate is green and every capability-map row lists its pages. The map's maintainer note names the feature pages and the registry. |
| F4 | The `/doc-coverage` pass (F-S) and the fifth rule (F-P) | The pass has been run once and its findings closed. The policy page, CLAUDE.md and `/adversarial-review` all state the rule, and "four rules" reads "five" wherever it appears. |

**Not in scope:** rewriting the guide chapters or runbooks the pages link to.
