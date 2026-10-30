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
- **The reconciler names an actor per row** (`Stored::actor`, from `Reconciler::apply_as` or a composition's
  principal). An effect applied with neither counts toward nobody's exposure.
- **The fleet scope is a gauge, not a count of permits:** one `sys/ae-exposure/{node}` summary per node, republished
  on the K2 tally heartbeat's tick. A node out of view whose last summary lists the actor is unknown coverage (its
  effects are still unresolved); a departed one is counted.
- **Shadow under `legacy` and `observe`** — deliberately unlike the governors, for which `legacy` consults nothing:
  attaching an exposure policy starts in shadow on a node that has not stepped the ladder.
- Not built: the consensus scope (a seat held until resolution has no fixed lease), a gate at a tuple take or a
  signal emit (no enforcement point there).

Pages touched: `concurrency/lock-order.md` row 68.
