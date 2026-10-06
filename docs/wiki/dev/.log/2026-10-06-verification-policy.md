## [2026-10-06] ingest | the verification policy — check what the change did not describe

**What:** `CLAUDE.md` § Verification policy; `docs/wiki/dev/testing/verification-policy.md` (new); the PR
template (new); `RELEASING.md` § 5b; `.claude/commands/adversarial-review.md` (new); `CONTRIBUTING.md`;
`scripts/check-test-discovery.sh` (new, in `make check` and CI); CI's Python and root integration-test steps
switched to discovery; `mycelium-py/tests/test_gateway.py` skips without `MYCELIUM_TEST_HOST`.

**Durable knowledge:**

- **Why the gaps survived green CI and repeated analysis:** every check was derived from the change's own
  description, so it confirmed what its author already thought of. The six gaps doc-coverage run 20 found sat at
  the edges a description never names — sibling functions, other doors, plan promises absent from the PR, files
  absent from an allow-list.
- **The four rules** (enumerate · evidence · discovery · adversarial review), each with where it is enforced.
- **An explicit list fails open, twice over.** CI's named Python files left five out; its named Rust integration
  tests left three out — after the 2026-09-26 review (F9) had found the same class and been answered by naming one
  more file. Discovery plus a gate that refuses a named file is the structural answer; all eight passed on first run.
