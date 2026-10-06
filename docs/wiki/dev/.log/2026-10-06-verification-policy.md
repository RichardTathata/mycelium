## [2026-10-06] ingest | the verification policy — check what the change did not describe

**What:** `CLAUDE.md` § Verification policy; `docs/wiki/dev/testing/verification-policy.md` (new); the PR
template (new); `RELEASING.md` § 5b; `.claude/commands/adversarial-review.md` (new); `CONTRIBUTING.md`;
`scripts/check-test-inventory.py` (new, in `make check` and CI); CI's Python and root integration-test steps
switched to discovery; the live suites moved to `tests/live/`, run as directories with `MYCELIUM_LIVE_REQUIRED`.

**Durable knowledge:**

- **Why the gaps survived green CI and repeated analysis:** every check was derived from the change's own
  description, so it confirmed what its author already thought of. The six gaps doc-coverage run 20 found sat at
  the edges a description never names — sibling functions, other doors, plan promises absent from the PR, files
  absent from an allow-list.
- **The four rules** (enumerate · evidence · discovery · adversarial review), each with where it is enforced.
- **An explicit list fails open, twice over.** CI's named Python files left five out; its named Rust integration
  tests left three out — after the 2026-09-26 review (F9) had found the same class and been answered by naming one
  more file. Discovery is the structural answer (the gate first proposed for it, refusing named files, is
  replaced — below); all eight passed on first run.
- **A gate that refuses a spelling is weaker than one that proves coverage.** The first version of rule 3's gate
  matched named test files; the PR's own adversarial review (rule 4, the first one run) bypassed it fourteen ways
  and found tests no step ran because no step enabled their feature — a class the gate could not see. The
  replacement inventories every target and maps it to a step with its features; it found five more uncovered
  targets on its first run. The policy's own first application caught the policy's own first implementation.
- **And the second review caught the second.** The inventory did not count library unit tests, read a
  non-feature cfg (`loom`) as needing nothing, and counted steps that never run tests (`if: false`, `--no-run`,
  `-- --list`, an `echo`); 20 of the reviewer's 38 mutations passed it, and `mycelium-core`'s `tls`-gated unit
  tests ran in no step. The rewrite inventories test *requirements* under every cfg gate and reads steps from the
  workflow YAML; its findings (core `tls`, wasm-host provisioner `gateway`, stem `llm`, three reason-node Python
  suites with no guard) run in CI now, and the reviewer's bypasses are a mutation suite the check runs against
  itself (`scripts/test-check-test-inventory.py`, 28 mutations). A checker is a claim too: it gets a test that
  fails when the checker is wrong.
