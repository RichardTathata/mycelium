## [2026-10-19] ingest | Two K2 test flakes, both test races, closed

- `an_abort_after_the_record_keeps_the_permit_counted`: the test waited for *any* journal record of alice's before
  aborting, but a refused seat claim also records a decision. Under load, the old 2 s lease margin refused every
  claim that lost a ballot. #648 fixed that margin; this test now loses one prepare every run, which pins the fix.
  It also asserts that alice's record is a permit before the abort, so a refused claim names itself instead of
  reading as `[]`.
- `an_aged_tally_entry_is_withdrawn`: a 300 ms window judged on the node's clock. Under load the test thread ran
  late, and the entry had already aged. The test now passes the entry's own instants and checks the exact boundary
  (kept at `t + w - 1`, withdrawn at `t + w`), with a window that no clock or tick can reach during the run.

Lessons:
- A test's wait for "a record exists" must name the record it means. A refusal is a record too.
- A test that asserts "not yet aged" against a live clock with a sub-second window asserts the scheduler's latency.
  Pass the instants instead.

Pages: none changed. Repo: `docs/plans/post-360-hardening.md` §3.6, `src/agent/fleet_history_tests.rs`.
