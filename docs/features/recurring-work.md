# Recurring work

<!-- A feature page links to its sources and copies nothing from them. See README.md in this directory. -->

## What it is

[Recurring work](../guide/00-concepts.md#recurring-work) is a `[[schedule]]` in a
[unit file](../guide/00-concepts.md#unit-file): work that should run once per period, held by one node at a time.
The holder is not assigned: every node running the schedule and matching its holder filter is a candidate, the
candidates order themselves by rendezvous, and the first holds for as long as its holding advertisement keeps
being re-asserted. Each run carries the idempotency key `sched/{id}/{slot}`, so the
[side-effect contract](../guide/00-concepts.md#side-effect-contract) makes two holders' runs of one slot one
effect, and a missed run is a decision with a reason in the [decision trace](../guide/00-concepts.md#decision-trace).

## When to use it, and when not

- **Use it when** a fleet should close a ledger, compact a store or post a report once per period whichever node
  is alive, and the work's effect can be deduplicated at its destination.
- **Not when** the work must never run twice: nothing here is exclusive, and a partition runs a slot on each side
  (`mycelium::schedule`'s module documentation); nor for a calendar — a period and a phase only.

## Turn it on

The section is `[[schedule]]` ([unit-file § schedule](../reference/unit-file.md#schedule)); the work is the embedding
application's.

- [`run_schedule`](../../src/schedule.rs) — `GossipAgent::run_schedule(&decl, work)` takes part in electing the
  holder and, while holding, calls `work` once per run; `work` returns `Err` to have the slot retried at the next
  tick.
- [`ScheduledRun`](../../src/schedule.rs) — one run: the slot, why it runs (on time, late, for a gap), and
  `operation_id`, the key an effect it causes carries.
- [`decide`](../../src/schedule.rs) — one tick's decision, pure.
- `two_holders_under_a_partition_give_one_effect_at_the_destination`, in
  [`src/schedule_tests.rs`](../../src/schedule_tests.rs): two nodes that cannot reach each other each hold and run
  the slot through their own reconciler, the destination applies it once, and healed, one holds.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The tests, in [`src/schedule_tests.rs`](../../src/schedule_tests.rs):
`two_holders_under_a_partition_give_one_effect_at_the_destination`,
`a_missed_run_is_a_decision_in_the_trace_with_its_reason` (a holder's clock stepped five periods through the seam;
the gap decided as run once or skipped, the run naming the decision its parent) and
`a_host_outside_the_holder_band_never_runs`; `wire-check`'s refusal of a schedule no unit can hold is
`a_schedule_without_a_declared_holder_band_is_refused`, in [`src/wire_check.rs`](../../src/wire_check.rs).

## What it guarantees, and what it does not

- The rules are [`sched.run` and `sched.missed`](../reference/rule-catalogue.md): the rendezvous winner among the
  live candidates runs the slot due; a gap is skipped, run late (at most 64, each under its own key) or run once.
- One effect per slot **only at a destination that deduplicates** on the run's key: two holders send it twice.
- A schedule with no recorded run starts at the slot due; nothing before the first run is counted missed. A gap
  is known from `sched/{id}/last` as this node has it, so a partition's two sides may each decide it.
- Not built, stated in the plan's Y2 row: a cron language beyond a period and a phase, a fleet-wide calendar, an
  SDK or gateway route that runs a schedule (`POST /gateway/units/declare` refuses one, 422), and a stem that runs
  one (it refuses one by name).

## Go deeper

- The design record is the plan's D15 and row Y2 ([`plans/adopter-surface.md`](../plans/adopter-surface.md)).
- The code: [`src/schedule.rs`](../../src/schedule.rs); the holder election is
  [`src/election.rs`](../../src/election.rs).
- No runbook: a schedule runs inside the application that embeds the node.
