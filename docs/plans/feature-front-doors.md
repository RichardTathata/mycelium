# Feature front doors — one short, checked entry page per feature

**Status:** adopted 2026-10-19, rev 0.1, **not started**. It begins after v2.36.0 ships. Decided with the
maintainer on 2026-10-19; no row below is delivered.

## 1 · The gap

A reader who wants one feature has no page that says what it is, how to turn it on, and where to see it
work. The [capability map](../capabilities.md) routes nine broad needs, such as *Constrain actions* or
*Coordinate work*, to guide chapters, examples and operations pages. It does not route individual features.
It has no column for the setting that enables a feature, and no plain one-paragraph account of what a
feature is.

Recent features show the gap. The activation egress proxy, fleet-wide history limits and inference routing
can each be found only in a long guide chapter, in [`unit-file.md`](../reference/unit-file.md), or in a
CHANGELOG entry. Nothing checks that such a page exists, or that it still agrees with the code.

## 2 · The page

Each feature gets one thin page, `docs/features/<feature>.md`, with these six sections in this order:

1. **What it is.** Two or three plain sentences, and a link to the concept in
   [`00-concepts.md`](../guide/00-concepts.md).
2. **When to use it, and when not.** One line each.
3. **Turn it on.** The smallest configuration or unit-file snippet that enables it, linked to its entry in
   [`configuration.md`](../reference/configuration.md) or [`unit-file.md`](../reference/unit-file.md).
4. **See it work.** One runnable example and the command that runs it.
5. **What it guarantees, and what it does not.** The guarantee id from the
   [guarantee catalogue](../reference/guarantee-catalogue.md), the plan row's *not built* items, and a link
   to [what-is-proven](../operations/what-is-proven.md).
6. **Go deeper.** The guide chapter, the operations runbook, the design record, and the code.

A page links to these sources and does not restate them, because a copy is what drifts. The capability map
becomes the index, and each of its rows lists its feature pages.

## 3 · Keeping the pages true

Three mechanisms keep the pages true, each for a different kind of check.

| # | Mechanism | Checks |
|---|---|---|
| F-G | **A CI gate** beside [`check-materials.py`](../../scripts/check-materials.py) | Every feature has a page. The feature list is derived from a registry, never kept by hand: the guarantee catalogue, the Cargo features and the unit-file sections, with stated exclusions. Every page has all six sections. Its links resolve. Its configuration keys exist in the reference. Its snippets are loaded by a test. Its example runs in a CI step. |
| F-S | **The `docs-analyse` skill gains a front-doors pass** | Judgement a script cannot make: whether the description is still true of the code, whether the example demonstrates the feature rather than merely running, whether the *not built* list matches the plan, and which features merged since the last pass lack a page. `/wiki-lint` stays wiki-only. The skill is read before it is changed. |
| F-P | **A verification-policy rule, and an `/adversarial-review` check** | A PR that adds or changes a feature adds or updates its front door, and the independent reviewer checks it. Drift is then caught at the PR, the way plan rows close on evidence (rule 2). |

## 4 · Delivery

| Row | Work | Exit |
|---|---|---|
| F1 | The template and three pilot pages: the activation egress proxy (rows L and M), fleet-wide history limits (K2), and inference routing (R) | Each pilot page reviewed independently against the code. The template is revised from what the pilots show. |
| F2 | The feature registry and the CI gate (F-G) | The gate fails on a feature with no page and on a page with a missing section, a dead key or an unrun example, with each plant shown failing. |
| F3 | The remaining pages, an estimated 30–40 | The gate is green. The capability map's rows list their pages. |
| F4 | The `docs-analyse` front-doors pass (F-S) and the policy rule (F-P) | The skill's pass has been run once and its findings closed. CLAUDE.md's verification policy and the `/adversarial-review` checks name the rule. |

**Not in scope:** rewriting the guide chapters or the runbooks the pages link to.
