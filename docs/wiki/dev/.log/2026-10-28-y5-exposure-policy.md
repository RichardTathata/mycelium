# 2026-10-28 — Y5: the causal exposure policy

Adopter-surface row Y5 (D18): a `[[rule]]` with an `exposure` table holds new dependent work at the gateway's doors
while the actor's unresolved closure — its effects whose outcome is not known — is past a size or an age threshold.

**Durable knowledge.**

- **Where the gate sits is the decisive rule.** The exposure gate is in `http::ae_decide` (the gateway's decision every
  door runs), not in `preflight_with_history` (the shared seam). A compensation is decided through the seam
  (`CompensationPermit::decide` → `preflight`), so it never meets the gate; a probe passes no evaluator; a shed and a
  rescue are governor actions. A future change that moves the gate into the seam breaks
  `a_compensation_is_admitted_while_a_new_dependent_call_is_held` — that is the plant.
- **The exposure is read before the per-actor history gate**, so the ledger's map (lock-order row 68) is never taken
  under another lock.
- **The input is the request's causal cone, not its actor** (#701's review, finding 1): a request extends an
  unresolved closure only when its `parent` names an unresolved effect's record or its `correlation` that effect's run.
  The first build attributed effects to actors and held all of an actor's work; independent work passing is the row's
  own promise (X9 leaves independent work untouched).
- **The fleet scope is a gauge, not a count of permits:** one `sys/ae-exposure/{node}` summary per node, its records'
  names digested, republished on the K2 tally heartbeat's tick and collected as K2 collects a non-peer's tally. A peer's
  `departed` mark is not read; a member out of view whose last summary lists what a request extends is unknown coverage
  until it departs or passes the history horizon.
- **`legacy` touches nothing**, as for every governor; `observe` is the shadow rung. A gate is counted only once the
  evaluator permitted.
- Not built: the consensus scope (a seat held until resolution has no fixed lease), a gate at a tuple take or a
  signal emit (no enforcement point there).

Pages touched: `concurrency/lock-order.md` row 68.
