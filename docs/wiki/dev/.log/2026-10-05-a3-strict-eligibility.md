## [2026-10-05] ingest | realignment repairs A3 — unknown history is never eligible

**What:** `src/mandate/eligibility.rs` (new: `TermHistory`, `Coverage`, `Origin`, `ChainedTerm`,
`eligible_strict`, `StrictEligibility`, `ready`), `examples/strict_eligibility.rs`, guide 21's
handover section, the changelog, the plan's A3 row, and this log.

**Durable knowledge:**

- **Coverage belongs to the source, sufficiency to the evaluator** (decision D4, refined by the
  reviewer). A `complete: true` flag only relocates the assumption; a chain of terms each naming its
  predecessor lets the source *verify* continuity, and an `Origin` plus the current head say where
  the vouched-for history begins and that it reaches the present.
- **Sufficiency is per rule.** Consecutive terms are decided by a run broken inside the verified
  suffix (or a suffix from genesis); cumulative tenure needs genesis or a baseline; cooling-off needs
  the suffix to cover the window. Exceeding a limit within what is visible is decided regardless.
- **Reading and eligibility are two conditions.** `ready` requires both; `Successor::admit` keeps
  checking only the first.
- **The lenient function stays as it is.** `handover::eligible` says in its own doc that an
  incomplete history passes; the strict one is additive, so no caller's meaning changed.
