## [2026-10-03] ingest | I7 — the governor and admission on the trace

**What:** `CoreCtx::decision_sink` (set once; `GossipAgent::with_decision_trace`), `membership.governed`
instrumented in `converge`, `signal.admission` partial in `deliver_locally` (refusals + sheds),
`TracePolicy::Partial`. Gates: core `admission_records_refusals_and_sheds_only_and_changes_no_delivery`,
root `test_membership_governor_with_a_decision_trace_decides_as_without_and_says_so`.

**Durable knowledge:** the sink lives on `CoreCtx`, not `TaskCtx`, because admission is a core function
with no view of the agent — the one place both layers can reach. A hot path is instrumented for its
*refusals* and the catalogue says so by policy (`Partial`), which is the honest middle between "every
admission recorded" (a record per signal) and "catalogue only" (a shed invisible). A hold record carries
the roll: the governor's "nothing happened" is a decision with a number behind it, and that number is
what a reader of an oscillation needs.
