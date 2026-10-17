## [2026-10-17] ingest | Policy over history (post-360 row K)

An action evaluator can decide on the actor's recent decisions: `ActionEvaluator::history_request` and
`evaluate_with_history`, `History::{View, Unavailable}`, `Rule::at_most(count, window_ms)` in the reference evaluator, and
`PolicyClause::AllowAtMost` with five cases in the contract suite. The history is this enforcement point's own (§5
decided by the user: per node, scope named), from `HistoryIndex` over the evidence journal. Found while testing: the
check and the count must be one step (a striped per-actor lock), and a view capped near the limit gets filled by the
actor's other records. A wrapping evaluator must forward the new methods, or its limited rules go `Indeterminate`.

Pages: `dev/dev.md` (AE section), `dev/concurrency/lock-order.md` (rows 61–62). Repo: guide 20, `deprecations.md` §47,
the plan's row K block (§3.5) and §5.
