## [2026-09-24] ingest | cohort budgets (Boundary H, item H6)

Up: [dev](../dev.md) · record `docs/design/knowledge-cohorts.md` §6 · code `src/knowledge/cohort_budget.rs` ·
lock-order row 42.

**Finding.** Per-caller caps let a colluding population overwhelm a provider together (the Hugging Face artifact-repo
outage). The federation edge meters per partner domain; nothing metered a population inside one domain.

**Change.**
- `CohortBudget::admit` gives an RAII `CohortSlot` or `AtCapacity`. It mirrors row 40's per-partner slot.
- Membership comes from the provider's trusted `CohortView`, keyed by the authenticated principal. There is no
  caller-supplied label.
- Multi-cohort callers need room in every pool, and a refusal takes nothing.
- Undeclared callers share one pool.
- Six tests.

**Kept honest.** Per instance, concurrency not rate, and only calls admitted through it. Refusals are not yet in the
rights ledger. It is not wired into any built-in provider: a provider opts in by calling `admit`.
