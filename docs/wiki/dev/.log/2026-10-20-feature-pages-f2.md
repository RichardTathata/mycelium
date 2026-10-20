## [2026-10-20] ingest | Feature pages F2: the registry and the gate

`docs/features/registry.toml` maps every id of six sources — the glossary's `<a id>` entries, the guarantee and
rule catalogues, the unit-file sections, every workspace crate's Cargo features, and guarantees a companion
registers in code (`GuaranteeDescriptor::new("<id>"` outside tests) — to a page, to `todo` (row F3), or to an
exclusion with its reason. `scripts/check-feature-pages.py` (in `check-positioning.sh`, so `make check` and CI's
documentation job) fails on an unmapped, doubly mapped or stale id, a page missing from the registry or a registry
page missing from disk, and per page on the six sections, links and anchors, code items, configuration rows, named
tests (a `#[test]` fn in the linked file, not excepted) and the example; CI's `test-coverage` job runs it with
`--executed` over the fetched logs, so a page cannot name a test no job ran. Mutation suite:
`scripts/test-check-feature-pages.py`, 30 plants and two baselines, listed in `test-universe`. The independent
review of #655 found the test-name rule too narrow (four-word names only) and too broad (any such name read as a
test), a page with no example able to name no test, the companion scan cut at the first test module, the slug
change stripping `<…>` inside code spans, and two narrow registry links; all fixed, and the gate's fixes planted. Its remaining
findings — completeness bounded by the sources, name-only checks — are stated in plan §4.2.

Decisions: P3 — companion guarantees are a sixth source read from code, not generated into the catalogue (the
catalogue's generator lives in `mycelium`, which cannot see its dependants, and a per-companion generator would be a
hand list again). P5 — the front-doors resolver's `slugs` reads `<a id>` and reduces a heading's link and inline
HTML; the gate imports it. `mycelium/sim` maps to the replay page (it turns on the node binary's recording), a
departure from the plan's stated exclusions; `mycelium-wasm-host/sim` is excluded.

Found: the egress page named `egress_credentials` and `egress` without the P9 marker; now linked lines the gate
checks. Script self-test audit: `check-hlc-current.sh`, `check-wiki-mutation-fence.sh` and
`check-stem-observations.py` run in CI with no mutation suite of their own (recorded, not fixed here).

Pages: `dev/examples.md` (the feature-pages paragraph). Repo: `docs/features/registry.toml` (new),
`docs/features/README.md` (the registry and the gate), `docs/plans/feature-pages.md` (rev 0.5, §4.2),
`scripts/check-front-doors.py`, `scripts/check-positioning.sh`, `Makefile`, `.github/workflows/ci.yml`.
