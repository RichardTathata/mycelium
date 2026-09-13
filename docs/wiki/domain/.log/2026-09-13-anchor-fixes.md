## [2026-09-13] lint | two stale code anchors corrected (doc-vs-code)

Up: [domain](../domain.md). Found while writing the v3 implementation plan from these pages.

- **management-as-intent.md** cited a generic `IntentReconciler<T>` in `src/agent/intent.rs`. No such type
  exists: the shipped plumbing is the `FleetIntent` trait with `publish_intent` / `read_fresh_intent` /
  `reconcile_intent` / `spawn_intent_reconciler`. The name was the design's (plans/elastic-sizing-intent-governed.md,
  a historical record, left as is). Page corrected.
- **plans/v3-contracts-axis.md §5 (item 2, Verified)** cited `federation_facts.rs` as federation's starting point.
  No such file: the federation edge is the `mycelium-agentfacts` crate (guide 17). Corrected in place.

Not a finding: the plan's `pub fn public_routes()` is phrased as a §9 deliverable, not as existing code. Rule
restated: a wiki page cites the identifier the code exports, not the design's working name.
