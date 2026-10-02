## [2026-10-02] ingest | doc-coverage run 18 — a setting that left the gateway open, and snippets that were not TOML

**What:** the coverage matrix re-audited over v2.15.1 → v2.18.0 (`docs/analysis/doc-coverage.md`, run 18):
five new rows (stem fleet, activation/serve, artifact delivery, the declaration schema, SDK unit files),
run 17's `~` cells re-audited, four literal failures fixed, one security defect fixed in code (its own PR).

**Durable knowledge:**
- **Load the snippet, don't read it.** The canonical unit-file example joined keys with `;` and was not
  TOML; the page that cited it had been called Clear. The module-doc example is now a test
  (`the_module_doc_unit_file_example_loads`, `src/capability_config.rs`).
- **A security setting works only in the build that compiles its consumer.** Token tables are enforced
  under `compliance` and were parsed in every build; see [security](../security.md) § the open gateway.
- **A "code gap" verdict cites the grep that found nothing** — run 17 filed commitments' retention verb as
  missing when `compact_log` existed.

**Pages touched:** `reference/unit-file.md` (new), `operations/capability-lifecycle.md`, `artifacts.md`,
`what-is-proven.md`, `shared-responsibility-matrix.md`, `companions.md`, `sso.md`, guides 00/02/10/13/18/20/21,
both SDK READMEs, the wasm-host README, the plan and `plans/README.md`, `examples/units/README.md`, the Makefile.
