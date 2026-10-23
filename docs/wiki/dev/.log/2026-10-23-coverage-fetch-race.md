## [2026-10-23] ingest | the test-coverage jobs' fetch race (ci-test-coverage.py --fetch)

- Three CI failures, all in `Test coverage (observed…)` jobs and none a missing test: cluster-suites run 39461823167
  attempt 1 ("Stem examples … had not finished", though in `needs`); main's ci.yml run 39439202186 on 2026-10-20 (a
  404 fetching a completed job's log); PR #661's run 39914821273 ("Test … had not finished"). The jobs listing and the
  log blob are eventually consistent for some seconds after a job completes; the coverage job starts in those seconds.
- Fix: `fetch` re-lists while any needed job reads unfinished and re-fetches a log that is not there, each inside one
  bounded wait (`CI_TEST_COVERAGE_WAIT_S` 120 s, `CI_TEST_COVERAGE_POLL_S` 10 s, env-tunable so the suite can shrink
  them); the failure messages are unchanged and still name the job. Three cases in `scripts/test-ci-test-coverage.py`
  (`fetch-waits-for-the-listing`, `fetch-retries-the-log`, `fetch-gives-up`) drive the script end to end through stub
  `gh` and `curl` on PATH; the first two fail against the unchanged script with the CI failures' own messages.
- Separately observed and **not fixed**: a *failed-only* re-run of a coverage job cannot pass (attempt 2 of the
  cluster-suites run: "the test universe was not listed"), because the copied sibling jobs' logs are not all
  fetchable from the new attempt. The operator remedy is a whole-workflow re-run; a script-side remedy would read
  logs from the previous attempt, which is a design question (which attempt is the record?) rather than a patch.

Lesson: a CI job that reads its siblings' state through the API, rather than through `needs`, must treat that
state as stale for seconds after the siblings complete; `needs` orders the jobs, not the API's view of them.

Pages: [`dev/testing/verification-policy.md`](../testing/verification-policy.md) (rule 3's observed coverage —
unchanged in substance). Repo: `scripts/ci-test-coverage.py`, `scripts/test-ci-test-coverage.py`, `CHANGELOG.md`.
