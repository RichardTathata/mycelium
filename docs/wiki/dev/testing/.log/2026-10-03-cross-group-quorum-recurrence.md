## [2026-10-03] ingest | `test_cross_group_propose_requires_all_group_quorums` recurred — instrumented, not loosened

**What:** the test failed in the v2.21.0 release PR's CI with `Timeout { ballots_tried: 3, votes_last_ballot: 0,
quorum_required: 1 }` on the *positive* (alpha-only) case — the second recurrence after 2026-09-28, which
said "if it recurs it is a flake to root-cause". Zero votes across three five-second ballots is not load.

**Ruled out by reading:** the quorum arithmetic (two members × 0.5 → 1); the ballot key (a fresh slot);
the probabilistic local shed (`deliver_locally` sheds only when the pair's own shards have fill, and a
two-node pair with one proposal has none); the vote channel (registered per call, capacity 512).

**Not ruled out:** the membership poll before the proposals does not assert — a 2 s gossip miss under a
loaded runner leaves the proposer's cached roster at one member for `health_check_interval_secs`, and the
proposer's own vote path on a `Groups`-scoped PROPOSE is what the next failure must show. The test now
asserts the membership wait with both views, and the positive case's failure message carries both
nodes' group views and peer lists. The release was rerun (one job) rather than the gate loosened; the next
recurrence is a root cause, not a retry.
