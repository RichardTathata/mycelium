# ingest — council-substrate Phase 2: group-per-council wiring (2026-08-15)

**Shipped:** `GitStoreConfig::for_group(dir, group)` — the group-per-council convention made
executable: one repo, one store instance per group, `subdir = councils/{group}`, commit messages
prefixed `wiki({group})`; a shard (set of councils) is the same constructor with a shard label.

**Gate (green):** `git_store_curator.rs::two_council_curators_share_one_repo_without_cross_scope_commits`
— two curators (norfolk/evesham), two independent agents/groups, ONE repo; concurrent proposals
drain into committed truth with **every commit touching exactly one council's subtree** and carrying
that council's prefix. The scoped-commit discipline holds under cross-instance concurrency because
it is the mechanism (per-path plumbing commits + the shared branch ref CAS), not a rule.

Phase 1 CI verdict (run 31902561047 on f6fcf8f): completed/success, all jobs green.
Remaining: Phases 3–5 (validator-as-curator-lint · bulk-ingest claim-check · work distribution).
