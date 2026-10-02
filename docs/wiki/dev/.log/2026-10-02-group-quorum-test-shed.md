## [2026-10-02] ingest | a test that assumed one emit is always admitted

**What:** `test_group_quorum_excludes_ex_member` failed once in the v2.18.1 release gate ("raw quorum
should be satisfied"), after four months green. Root cause, not a retry: `deliver_locally` sheds local
delivery with probability equal to the gossip shards' fill, and shedding happens **before** the sender is
recorded. The test's agent is never started, so the frame its `join_group` KV write queued is never
drained — fill 1/1024, a shed about one run in a thousand. The test now emits until admitted (bounded at
50) and says why; its claim is which admitted senders `group_quorum` counts, not that one emit lands.

**Durable knowledge:** an unstarted agent's gossip shards fill and never drain, so every KV write before an
`emit` raises the local shed probability. A test that asserts on a single emit's local delivery must
either start the agent, emit to `Individual` scope (exempt from shedding), or emit until admitted. A sweep
of the suites found no other test with the pattern.

**Noted, not changed:** shedding is proportional with no floor, so an idle node with one queued frame sheds
~0.1% of local non-`Individual` signals. Whether a small threshold belongs there is a design question.

**A second one, found by this PR's own CI.** `the_release_gates_choreography_over_the_transport` asserted
`gw2.peers().len() == 2` immediately after `assert_never_merged` passed, with no wait: gw2 joins after gw1
is shut down and can learn the dead gateway from a1/a2 by peer exchange before the failure detector has
evicted it — left 3, right 2, the third an A node. The assertion now checks every peer is a domain-A node
(the claim), then waits up to 30 s for the eviction before the exact count. Same lesson as the
lock-acquire flakes in [testing](../testing/testing.md): a count asserted without a structural poll is a
timing assumption.
