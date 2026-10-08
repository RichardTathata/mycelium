# 2026-10-08 — a local emit is not a delivery (#568, PR #569)

`test_commit_conflict_tripwire` failed once under the strict compliance gate: its single forged COMMIT was shed. Local
delivery of a Cluster/Group signal is admitted with probability `1 − fill` (`ops::deliver_locally`), and when
`cluster_propose` returns the proposer's own COMMIT is still in the listener's 256-slot queue (fill 1/256, measured by the
review). The test re-emits until the tripwire fires, takes its baseline once the count is stable, and checks the
idempotent COMMIT only after a second subscriber saw it delivered; `the_tripwire_holds_under_a_loaded_signal_queue`
plants the load (the single-emit version fails it 3/3). The review's rule-1 sweep found `test_signal_group_admitted_when_member`
with the 2026-10-02 pattern (unstarted agent, `join_group`'s frame in a shard, fill 1/1024) that day's sweep missed — fixed
the same way. Page: `testing/testing.md` § A local emit is not a delivery.
