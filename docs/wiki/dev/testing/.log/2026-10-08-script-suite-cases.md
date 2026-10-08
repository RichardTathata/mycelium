## [2026-10-08] ingest | rule 3 observed for script-style suites

- Convention: a suite's `--list` prints `@@case-list@@ <suite>::<case>` per case without running anything; a run
  prints `@@case@@ <suite>::<case>` as each case starts (searched anywhere on a line, so a Docker runner's
  container-name prefix does not hide it). `scripts/ci-test-coverage.py` keys them `script <suite>::<case>`.
- `ci.yml`'s `test-universe` lists the in-run suites (`scripts/test-*` but the inventory's mutation suite, the
  example wire-check, seven `ci_smoke.sh`, the LangGraph ladder); `cluster-suites.yml` lists the Docker suites per
  job and checks them in its own `case-coverage` job (`--scripts-only`), since ci.yml's job never reads that
  workflow.
- Pages: `verification-policy.md` (what the job sees and cannot), `cluster-suites.md` (the per-case check);
  `docs/operations/what-is-proven.md`'s universe row.
