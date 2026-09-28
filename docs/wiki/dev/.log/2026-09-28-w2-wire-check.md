# 2026-09-28 — W2: the offline wire-check, and the lifecycle page (A2, L1, L2, R1's checker half)

**What:** `src/wire_check.rs` + `mycelium wire-check` in `src/main.rs`. Pure over `Unit`s
(`NodeCapabilityConfig` per file) and `ArtifactDescription`s; runs `CapFilter::matches` (and the
crate-private `matches_ignoring_schema` for the schema-only case); eleven finding kinds keyed by stable
text; text, JSON (`mycelium.design/declaration/1`, `revision`) and DOT renderers; exit 0/1/2.
Fixtures `tests/fixtures/units/{coop,artifacts,unwired}` and `tests/wire_check_fixtures.rs` (golden
JSON, `UPDATE_GOLDEN=1` to regenerate). The operator page `docs/operations/capability-lifecycle.md`
(D16) with pointers from `deployment.md`, `artifacts.md`, guide 02 and the ops index.

**Durable knowledge:**
- **A2 reads artifact *descriptions*, not the manifest.** `mycelium` cannot depend on
  `mycelium-wasm-host` (the dependency runs the other way), so the checker's provisioning input is
  D10's reviewable TOML (`kind`, `[provides]`, `[requires]{disk_bytes, mem_bytes}`); the hosting
  footprint is `disk + mem`. CI keeps description and manifest equal once A1 exists.
- **A group's `provides` count as offers only when its filter matches some declared unit capability**
  — the design-time reading of "a group asserts its provides when it has members".
- `wire_check::check` reads nothing; every filesystem call is in `main.rs`'s subcommand, admitted in
  the replay inventory §2.4 (the `src/main.rs` baseline row rises to 3) — a design-time tool has no run
  to replay.
- The eleven kinds and their severities are in the module doc's table; `single provider` is only
  raised for a `[[requirement]]`, not for a group's `requires` (a group edge is already named by
  `group requires unmet` when empty).
- W1's CI run showed `test_cross_group_propose_requires_all_group_quorums` failing once under
  `CI_RETEST_STRICT=1` and passing on the strict retest — a consensus test unrelated to the change,
  rerun rather than papered over; if it recurs it is a flake to root-cause, not a gate to loosen.

**Not built:** the `--units` startup path (R1's runtime half, D18); W3 schema-directory awareness;
W4 the authority overlay; W5 was the DOT renderer and is in (`--format dot`), the guide-12 paragraph
is not yet.

**Pages touched:** `companions/companions.md` (pointer); the plan's W2/A2/L1/L2/R1 rows;
`CHANGELOG.md`; `docs/analysis/doc-coverage.md` (row *capability lifecycle*).
