# 2026-10-09 — doc-coverage run 22's three code gaps

- **The stem and the node binary stop on SIGTERM, from the first instant.** `mycelium-stem` awaited `ctrl_c()` only, so
  `docker stop` killed it without its `--trace-dir` output. Both binaries also registered their handler only once
  startup was done; a SIGTERM during the bind took the default action (the review reproduced it, 2 in 40). Handlers
  are now created at the top of the runtime and awaited later (`ShutdownSignal`); the node skips that in `-i` mode,
  where a handler nobody awaits would swallow Ctrl-C. Pinned by `mycelium-wasm-host/tests/stem_shutdown.rs`.
- **A consensus timeout is labelled by who answered, not by phase.** The first cut labelled any prepare-phase
  timeout `promise_short`; since 2.30.0 every attempt starts there, so a partition became `promise_short` — and in
  `propose` the proposer's own vote had long made a partition `quorum_short`. `timeout_reason(LastAttempt, others)`:
  `no_voters` · `promise_short` · `contended` · `blocked` · `quorum_short`. Pinned end to end by
  `a_proposer_nobody_answers_times_out_as_no_voters` (a thread-local `metrics` recorder).
- **The checkpointer names the route's 403 `refused`** (`langgraph-checkpoint-mycelium` 0.3.1).
- **The second review of #579** found a refusal in the voting phase labelled `no_voters` (now `contended`), and a
  bug older than this PR: `cross_propose` cleared each group's count per ballot but kept the set of voters seen, so a
  voter from an earlier ballot never counted again and a retry could not reach quorum (`CrossState::begin_attempt`,
  `a_cross_group_retry_counts_its_voters_afresh`). The cross tally can include this node's own looped-back vote.
