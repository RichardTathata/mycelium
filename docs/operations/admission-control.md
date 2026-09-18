# Admission control at the companions' queues — the runbook

*v3 item 4 PR 5 · `docs/design/adaptive-stability.md` §1 (service objectives), §3 (one owner per deficit), §6.*

The record's rule: **rejected work is reported beside completions, because a rejection is a visible
outcome, not a silence.** The companion that owns a queue owns its deficit, enforces its own bound at
the point where work is admitted, and counts what it turned away where an operator can read it. These
bounds are **self-imposed** (Tier B in the record's vocabulary): the primary keeps its own queue within
them; nothing here is a fleet ceiling, and nothing claims exclusive rights.

## The tuple space (`mycelium-tuple-space`)

| Knob | Where | Meaning |
|---|---|---|
| `TupleConfig.high_watermark` (default 500) | per stage, at the primary | a `put` that finds the stage's depth at or past it is refused — `TupleError::Backpressure { retry_after_ms }` — or, under `BackpressureMode::Block(limit)`, retried with backoff until `limit` |
| `TupleSpace::admission(stage)` | **the primary only** (`Some`); a secondary answers `None` | per stage: `admitted` (puts accepted), `rejected` (puts refused at the watermark), `taken` (items handed to a worker), `high_watermark`. Cumulative since the primary started |
| `sys/tuple/{node}/{ns}/stage/{stage}/rejected_total` | the metrics writer, beside `put_total`, `take_total`, `hot_total`, `depth`, `inflight` | the same count, published on the metrics cadence |
| `sys/tuple/{node}/{ns}/pressure/{stage}` | the backpressure pheromone (0.7 hysteresis) | *that* the stage is at its watermark — the signal producers should read before they retry |

**Reading it.** A stage whose `rejected` climbs while `taken` does not is a bound doing its job against a
producer that is not listening to the pheromone; a stage whose `rejected` climbs *and* `taken` climbs is
under-provisioned — more takers, or a deeper watermark if the memory is there. `admission()` is `None`
on a secondary because the deficit is the primary's; ask the primary (the depth RPC still answers
everywhere, but it does not carry the count — its encoding is fixed).

## The blackboard (`mycelium-blackboard`)

| Knob | Where | Meaning |
|---|---|---|
| `BoardConfig.high_watermark: Option<u64>` (default `None`, unbounded — the behaviour before v2.8.0) | the claimable pool, at the primary | a `post` that finds `available` at or past it is refused — `BlackboardError::Backpressure { available, high_watermark }` — and crosses the RPC as itself |
| `BoardStats.rejected` | `Blackboard::stats()`, beside `posted`, `claimed`, `acked`, `released`, `requeued` | the count of refused posts |

**What the bound is not.** Replication (`post_with_id`) and WAL replay are not admission and never refuse
— a bounded board still recovers everything it held. The check and the insert are two steps, so a burst
can briefly overshoot the watermark by the number of concurrent posters: a self-imposed bound, not a hard
one, and the record says which tier that is. A pre-2.8 client reading a refusal from a 2.8 primary sees a
generic `Rpc` error — the same refusal with less detail, never a false success.

## The example

`cargo run -p mycelium-tuple-space --example admission` — a burst past the watermark with nobody draining:
the excess refused and counted, the stage drained, a second burst admitted, the refusals still on the
record. Exits 0 with `All assertions passed`.

## What this does not give you

A fleet-wide bound (that is the rights ledger, §4 of the record — `Provisioner::with_install_rights` is
the first user), fair scheduling among producers (nothing here prioritises), or a governor that reacts to
the count (the workload probe consumes depth, §6; acting on `rejected` is a policy the operator writes).
