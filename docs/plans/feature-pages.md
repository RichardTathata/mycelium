# Feature pages — one short, checked entry page per feature

**Status:** in progress, rev 0.5. Agreed with the maintainer on 2026-10-19; work began after v2.36.0 shipped.
**F1 is delivered** (PR #654): the template and three pilot pages, each independently reviewed against the code (two
rounds and a confirmation), and the template revised from them (§4.1). **F2 is delivered** (PR #655): the feature
registry and the gate, each check shown failing with a planted fault (§4.2). F3 is next. Rev 0.2 answered the
independent review of PR #649; rev 0.3 recorded what the pilots taught; rev 0.4 records the decision on each (§4.1);
rev 0.5 records F2 and its decisions (P3, P5).

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
| F-G | **A CI gate**, a sibling script of [`check-materials.py`](../../scripts/check-materials.py) and [`check-front-doors.py`](../../scripts/check-front-doors.py) (which checks the repository's entry pages — the name this plan avoids) that shares their link and anchor resolution | It reads a checked-in **feature registry**: every id from every source maps to a page, or to an exclusion with a stated reason, and the gate fails on any unmapped id. This is the pattern of the reachability registry and the lock-order table, so a new id cannot slip through by being absent from a hand list. **Sources:** the concepts glossary anchors in `00-concepts.md`, the guarantee catalogue, the rule catalogue, the unit-file sections, the Cargo features of `mycelium`, `mycelium-core`, `mycelium-wasm-host` and the companions (as built: every workspace crate's), and — decided in F2 (§4.1 P3) — the guarantees a companion registers in code. **Many-to-one mapping is expected**, because most guarantee ids are settings of a feature. For example, `tls` maps to `mesh.tls`, `gw.tls` and `id.*`, and `[activation.confine.egress]` maps to `egress.credentials` and `prov.activation_egress`. **Stated exclusions:** build and test features (`cli`, `sim`, `fuzz-internals`, `test-util`, `test-frame-filter`) and declaration vocabulary (`principal`, `[[capability]]`, `[[requirement]]`). **Each page is checked for:** all six sections, links that resolve, settings that exist in the reference, and an example listed in the example index with its matrix status (✓, ✓ᵖ or `·` — compiled in CI but not run, as for a browser or manual demo), so the gate does not require a CI run that cannot exist. |
| F-S | **[`/doc-coverage`](../../.claude/commands/doc-coverage.md) gains a feature-pages pass** | It covers the judgement a script cannot make: whether the description is still true of the code, whether the example demonstrates the feature rather than merely running, whether the *not built* links match the plan, and which features merged since the last pass lack a page. It already audits documentation against code, and its rule that "presence is not sufficiency" is the same test. The skill is read before it is changed. `/wiki-lint` gains no feature-pages pass. |
| F-P | **A fifth verification-policy rule**, in [`verification-policy.md`](../wiki/dev/testing/verification-policy.md), in CLAUDE.md's summary of it, and in the `/adversarial-review` checks | A PR that adds or changes a feature adds or updates its page and its registry entry, and the independent reviewer checks both. Drift is then caught at the PR, the same way rule 2 closes plan rows on evidence. Adding a rule to a policy marked non-negotiable is the maintainer's decision, agreed on 2026-10-19. |

## 4 · Delivery

| Row | Work | Exit |
|---|---|---|
| F1 — *delivered (#654): the template ([`docs/features/README.md`](../features/README.md), [`_template.md`](../features/_template.md)), revised from the pilots (§4.1), and the three pilots — [activation egress proxy](../features/activation-egress-proxy.md), [inference routing](../features/inference-routing.md) and [fleet-wide history limits](../features/fleet-history-limits.md) — each independently reviewed against the code (two rounds and a confirmation)* | The template and three pilot pages: the activation egress proxy (rows L and M), fleet-wide history limits (K2) and inference routing (R) | Each pilot page has an independent review against the code, and the template is revised from what the pilots show. |
| F2 — *delivered (#655): [`docs/features/registry.toml`](../features/registry.toml), [`scripts/check-feature-pages.py`](../../scripts/check-feature-pages.py) and its mutation suite [`scripts/test-check-feature-pages.py`](../../scripts/test-check-feature-pages.py); evidence in §4.2* | The feature registry and the gate (F-G) | The gate fails on an unmapped id, on a page with a missing section, on a dead link or setting, and on an example absent from the index. Each of these is shown failing with a planted fault. |
| F3 | The remaining pages, an estimated 30 to 40 | The gate is green and every capability-map row lists its pages. The map's maintainer note names the feature pages and the registry. |
| F4 | The `/doc-coverage` pass (F-S) and the fifth rule (F-P) | The pass has been run once and its findings closed. The policy page, CLAUDE.md and `/adversarial-review` all state the rule, and "four rules" reads "five" wherever it appears. |

**Not in scope:** rewriting the guide chapters or runbooks the pages link to.

### 4.1 · Template revisions (F1)

What the three pilots taught, and the decision on each (F1's exit). P1–P8 came from the first two pilots, P9 from
the K2 page; the fixed-marker forms of P1, P2 and P9 were drafted by the independent review of #654. The adopted rows
are folded into the [front page](../features/README.md) and the [skeleton](../features/_template.md).

| # | Found | Revision | Decision |
|---|---|---|---|
| P1 | Inference routing has no configuration field or unit-file section: it is turned on in code (`reason_router_classified`, `DataClassPolicy`, `Rule::for_class`/`at_locality`, `ModelProfile::in_domain`). §2's *Turn it on* assumed a setting. | *Turn it on* for a feature enabled in code opens with the fixed sentence "This feature is turned on in code", then lists one item per line, its name in backticks linked to the file that defines it — the last path segment as `fn`, `struct`, `enum`, `trait` or `const` in the linked file — and ends with the test that runs that code: its name in backticks, a `fn <name>` in the linked file (checked pre-push), listed by CI's test universe and matched by no entry of `scripts/test-coverage-exceptions.txt` (checked in CI). A test that reads the page is preferred; a hand-kept copy (as `guide_20_routing_policy` is) is recorded as such. The F-G gate checks the sentence, each item's last path segment in its linked file, and the test as above. | **Adopted.** |
| P2 | Neither pilot has an example in the example index — both are shown end to end only by tests (`mycelium-wasm-host/tests/stem_sighup.rs` and an `activation::tests` case; `mycelium-reason/tests/gateway.rs`). | *See it work* opens with the fixed sentence "No runnable example exists" and links the end-to-end test: its name in backticks, a `fn <name>` in the linked file, listed by CI's test universe and matched by no entry of `scripts/test-coverage-exceptions.txt`, so it cannot name a test CI never runs. The feature registry records `example: none`, and the registry also fails if the example index later gains a row for a feature recorded as `example: none`. F3 may instead add examples. | **Adopted.** |
| P3 | `stem.activation_egress` and `stem.activation_confined` are registered by `mycelium-stem`, not the core, so they are not in the guarantee catalogue; there is nothing to link them to but `unit-file.md`'s prose and the code. | Either the guarantee catalogue gains the companion-registered guarantees, or §2 allows linking such an id to the page that states its states. The F-G registry's sources must include companion-registered guarantee ids either way, or these two are invisible to it. | **Adopted in its interim form:** a companion-registered id links to where its states are described. **Decided in F2: a sixth registry source read from code, not generation into the catalogue.** The catalogue is generated by `mycelium`'s own lib test from `core_guarantees()`, and `mycelium` cannot see the crates that depend on it; generating companion ids would need a second generator in each companion writing beside it, and the catalogue's completeness would then rest on each companion remembering to add one — the hand list the registry exists to replace. The gate instead reads every `GuaranteeDescriptor::new("<id>"` outside tests in every workspace crate, so a companion's new guarantee is unmapped the day it lands (plant `unmapped-companion-guarantee`). The pages keep the interim form's link. |
| P4 | Several targets have no per-entry anchor: the guarantee catalogue's descriptor table, the what-is-proven ledger's rows, `configuration.md`'s field table, and the plan rows (only §3.3 and §3.5 have anchors). A page can link only to the table and name the row. | Give the catalogues' rows, the ledger's rows and the configuration fields `<a id>` anchors (generated where the page is generated), so a page and the gate can point at one entry. Until then a page names the row beside the table link. | **Adopted.** The anchors themselves are F2/F3 work; until they exist a page names the row beside the table link. |
| P5 | The two scripts F-G is to share resolution with do not cover the pages: `scripts/check-materials.py` resolves files only, not anchors, and its scope excludes `docs/features/`; `scripts/check-front-doors.py`'s `slugs` resolves heading anchors but not `<a id>` ones, so every glossary link (`00-concepts.md#activation`, `#data-class`, …) reads as dead to it. Checking the pilots' links with a resolver that does both found one dead anchor already in the repository (`unit-file.md` → `crown-jewel.md#credentials-attached-at-egress`; the heading's slug carries its suffix), fixed here. | F-G resolves anchors from headings **and** `<a id>`, and scopes `docs/features/`; the shared resolver gains `<a id>` rather than F-G inheriting either gap. | **Deferred to F2; delivered there:** `slugs` in `scripts/check-front-doors.py`, which the gate imports, now reads `<a id>`/`<a name>` anchors and reduces a heading's link and inline HTML as GitHub does; the gate resolves every link in `docs/features/` with it (plants `dead-anchor`, `dead-glossary-anchor`, `dead-link`). |
| P6 | Neither pilot has a design record; the plan row (post-360 L, M, R) is the record. | *Go deeper* says "no design record; the plan row is the record" when that is so, instead of leaving the slot empty. | **Adopted.** |
| P7 | The glossary has no entry for either feature by name: the egress proxy lives inside `#activation` and `#credential-binding`, and locality shares `#data-class`. | *What it is* may link more than one glossary entry. Whether each feature gets an entry of its own is F3's question, not the template's. | **Adopted.** |
| P8 | Section 5 needs to say the headline limit (Landlock's open port; a declared, never detected class) or a reader leaves with the wrong idea, which strains *link, do not copy*. | Allow one short clause per headline limit in sections 2 and 5, with the link to the place that states it in full. The front page's *Link, do not copy* already says so. | **Adopted.** |
| P9 | K2 is turned on in code, but how it counts depends on configuration fields too (`control_max_staleness_ms`, `control_min_peers_heard`, `max_clock_drift_ms`, `tls`, `require_identity_proofs`, `consensus_require_electorate`). Its guide snippet has no test that reads or copies it, so the page names a test that builds the same policy itself. Writing *Turn it on* also showed that guide 20 did not say the exact scope needs a consensus listener on the voting nodes; fixed in guide 20 here (*Which electorate decides a seat*). | A configuration field is its own line, `name` in backticks linked to `configuration.md#the-fields`; the gate checks a row `| name |` exists in that table. A guide snippet with no test is recorded as such on the page, and the gap goes to the `/doc-coverage` pass (F-S) rather than being fixed on the feature page. | **Adopted.** |

### 4.2 · F2's exit, on evidence

| Promise | Delivered by | Shown failing (plant in `scripts/test-check-feature-pages.py`) |
|---|---|---|
| Every id of every source maps to a page or an exclusion with a reason; the gate fails on an unmapped id | `check_registry` over `source_ids` (six sources); `docs/features/registry.toml` | `unmapped-concept`, `unmapped-cargo-feature`, `unmapped-companion-guarantee`, `unmapped-unit-section`; also `stale-registry-id`, `mapped-twice`, `exclusion-without-reason` |
| A registry page that does not exist, and a page not in the registry, fail | `check_registry` | `registry-page-missing`, `page-not-in-registry` |
| A page with a missing section fails (all six, in order, by heading) | `check_page`, `SECTIONS` | `missing-section`, `sections-out-of-order` |
| A dead link fails, heading slugs and `<a id>` both (P5) | `check_links` with front-doors' `resolve`/`slugs` | `dead-anchor`, `dead-glossary-anchor`, `dead-link` |
| A dead setting fails: a code item that is not a `fn`/`struct`/`enum`/`trait`/`const` (P1), a configuration field with no row (P9) | `check_page` (`defines`, `config_fields`) | `absent-fn`, `missing-config-row` |
| A named test is a `#[test]` `fn` in its linked file and not excepted (P1, P2); in CI, executed | `named_tests`, `defines_test`, `excepted`; `check_executed` in the `test-coverage` job | `absent-test`, `not-a-test`, `excepted-test`, `in-code-without-test`, `test-not-executed` (with `executed-baseline`) |
| An example absent from the index fails; an index row for an `example: none` feature fails (P2) | `check_page`, `check_examples` | `example-absent-from-index`, `example-for-none-feature`, `none-without-sentence` |
| Every page links the ids the registry gives it | `links_id` | `concept-not-linked`, `guarantee-not-linked` |

**Not built in F2:** per-entry anchors for the catalogues, the ledger and the configuration table (P4, F3's); the
capability map's maintainer note naming the registry (F3's exit). **A departure from §3's stated exclusions:**
`mycelium/sim` and `mycelium-core/sim` map to the replay page rather than to an exclusion, because `sim` is also
what makes the node binary record a replay bundle; `mycelium-wasm-host`'s `sim`, which only turns on that crate's
test seams, is excluded.

**A link to re-point at release.** The K2 page links `deprecations.md` §55 by its heading's anchor, which carries
*unreleased*; the release that renames the heading must re-point that link in `docs/features/fleet-history-limits.md`.
