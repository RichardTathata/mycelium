## [2026-10-23] ingest | the test-coverage jobs' fetch race (ci-test-coverage.py --fetch)

- Three CI failures, all in `Test coverage (observed…)` jobs and none a missing test. Two were the API's lag:
  cluster-suites run 39461823167 attempt 1 ("Stem examples … had not finished", 19 s after it completed, though in
  `needs`) and PR #661's run 39914821273 ("Test … had not finished", 23 s after). The jobs listing and the log blob
  are eventually consistent for some seconds after a job completes; the coverage job starts in those seconds. The
  third, main's run 39439202186 on 2026-10-20, was **not** lag — the independent review of #662 read the API: `WASM
  host confinement (macOS Seatbelt)` was cancelled in the macOS runner queue with zero steps, fourteen minutes before
  the coverage job started, and was outside the job's `needs`; a job that never ran has no log, ever, and the
  script fetched every completed job's log, `needs` or not.
- Fix: `fetch` re-lists while any needed job reads unfinished and re-fetches a log that is not there, each inside one
  bounded wait (`CI_TEST_COVERAGE_WAIT_S` 120 s, `CI_TEST_COVERAGE_POLL_S` 10 s, env-tunable so the suite can shrink
  them, a non-number refused by name); a job that completed `cancelled` or `skipped` is not fetched (its tests, if
  listed, then read as not executed — the truth); `wasm-host-macos` joins the ci.yml coverage job's `needs`. The
  failure messages still name the job. Five cases in `scripts/test-ci-test-coverage.py` drive the script end to end
  through stub `gh` and `curl` on PATH; four fail against the unchanged script (two with the CI failures' own
  messages), and `fetch-gives-up` pins that the wait is polled, not spun.
- Separately observed and **not fixed**: a *failed-only* re-run of a coverage job cannot pass (attempt 2 of the
  cluster-suites run: "the test universe was not listed"), because the copied sibling jobs' logs are not all
  fetchable from the new attempt. The operator remedy is a whole-workflow re-run; a script-side remedy would read
  logs from the previous attempt, which is a design question (which attempt is the record?) rather than a patch.

Lesson: a CI job that reads its siblings' state through the API, rather than through `needs`, must treat that
state as stale for seconds after the siblings complete; `needs` orders the jobs, not the API's view of them. And
three failures with one message are not one cause: the first draft of this fix called all three lag, and the review
read each run's API record before agreeing.

Pages: [`dev/testing/verification-policy.md`](../testing/verification-policy.md) (rule 3's observed coverage —
unchanged in substance). Repo: `scripts/ci-test-coverage.py`, `scripts/test-ci-test-coverage.py`, `CHANGELOG.md`.
