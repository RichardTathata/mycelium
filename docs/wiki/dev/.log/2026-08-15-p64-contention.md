# ingest — P6.4: the measured contention run (2026-08-15)

**Shipped + measured.** The gate the plan demanded before any council-scale claim:
`ten_councils_contend_without_spurious_failures_measured` — 10 councils × 5 concurrent per-meeting
batches, both topologies, zero spurious failures. **Numbers (macOS dev machine): (a) shared
checkout 9.0s = 5.5 batches/s (the discouraged ceiling); (b) deployed clone-per-node → one origin
16.5s = 3.0 batches/s under maximum cross-council push contention.** Recorded in the plan; the
391 extrapolation deliberately stays unclaimed (one machine, ten councils).

**The measurement discipline worked — four real defects surfaced before the gate passed:**
1. Concurrent `open()`s raced `git init` on a shared checkout → init now tolerates losing the race.
2. Private temp index/worktree names used a **per-instance** counter → N stores in one process
   collided on the same `.mycelium-index.{pid}.{seq}` paths, corrupting each other's private
   indexes (and temp renames could cross-deliver content). Now a process-global counter.
3. **`merge-tree --write-tree` falsified**: ten councils cold-starting one empty origin create
   *unrelated root commits* — no merge base exists, the merge errors. Replaced with the
   **worktree-free subtree splice** (their tree + our scope's files, `ls-tree -z` →
   `update-index -z --index-info`): no ancestor needed, and merge-CORRECT for scoped stores
   because my subdir is mine alone (the topology rule). Documented caveat: a clone hosting many
   groups publishes the whole branch history; clone-per-group is the deployed shape.
4. The first-cut linear 24ms-cap backoff **starved** writers under the burst — the CAS window
   spans the ~100ms commit build, so losers retried straight into the storm. Now jittered
   **exponential** backoff (10→800ms cap): contention queues, never errors.

The sharpening, again: a bounded-retry loop without contention-scaled backoff is a starvation bug
wearing a robustness costume — and only the measured run under a real burst exposes it.

All suites green (git_store 25 · curator 6 · exactly-once · control-plane 28); clippy clean.
Remaining: P6.5 (entity-format codec) + P6.6 (lower tier).
