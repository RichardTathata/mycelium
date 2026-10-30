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
  [`src/schedule_tests.rs`](../../src/schedule_tests.rs): two connected nodes elect one holder; cut apart, the other
  holds once the lease leaves its view, each runs the slot through its own reconciler and the destination applies
  it once; healed, one holds.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The tests, in [`src/schedule_tests.rs`](../../src/schedule_tests.rs):
`two_holders_under_a_partition_give_one_effect_at_the_destination` (a real link cut at the frame layer: the other
side holds once the holder's lease has left its view, never before),
`a_missed_run_is_a_decision_in_the_trace_with_its_reason` (a holder's clock stepped five periods through the seam;
the gap decided as run once or skipped, the run naming the decision its parent) and
`a_host_outside_the_holder_band_never_runs`; `wire-check`'s refusal of a schedule no unit can hold is
`a_schedule_without_a_declared_holder_band_is_refused`, in [`src/wire_check.rs`](../../src/wire_check.rs).

## What it guarantees, and what it does not

- The rules are [`sched.run` and `sched.missed`](../reference/rule-catalogue.md): the rendezvous winner among the
  live candidates runs the slot due; a gap is skipped, run late (at most 64, each under its own key) or run once.
- One effect per slot **only at a destination that deduplicates** on the run's key, with content that depends on
  the key alone: two holders send it twice.
- A loop waits three lease refreshes before it may hold; a run gets one period, and work that overruns it is
  abandoned, traced (`work_overran`) and the holding withdrawn. A loop whose thread is blocked by synchronous work
  keeps its advertisements re-asserted by the node's persist task: such work belongs on a blocking thread.
- The fleet's last run is the highest `sched/{id}/last/{node}` under the schedule's period and phase, never lowered;
  a run recorded ahead of a holder's clock (a fast clock elsewhere) is said once (`last_ahead`) and nothing runs
  until the clock reaches it. A schedule with no recorded run starts at the slot due. Holders whose clocks disagree
  across a slot boundary compute two slots — two keys.
- One id is one schedule: candidates that declare another period or phase hold nowhere (`declaration_mismatch`) —
  so changing a running schedule's period or phase stops it fleet-wide for the rollout; roll a change under a new id,
  or stop it everywhere and start it again.
- A recorded run more than 64 slots ahead of a holder's clock is ignored and said once; `last` entries are one per
  node that ever ran the schedule, kept for good, and no route clears one.
- A holder whose work always overruns re-holds every tick and never hands over; the trace says `work_overran` each
  time. The warm-up is a delay, not a convergence check.
- Not built, stated in the plan's Y2 row: a cron language beyond a period and a phase, a fleet-wide calendar, an
  SDK or gateway route that runs a schedule (`POST /gateway/units/declare` refuses one, 422), a stem that runs
  one (it refuses one by name), and a schedule class in `mycelium test --faults`.
- A recording replays it: the loop's clock, tick and deadline go through the seams, and the checked-in recording
  `tests/replay-corpus/y2-schedule-run/` (one node holding `close`, deciding a missed run under `run_once`, a run
  that overruns its deadline, then the gap's run and the on-time run) is replayed decision for decision in another
  process by `tests/schedule_replay.rs`, run in CI. `run_late`, `skip` and a failed run are covered by unit tests,
  not a recording.

## Go deeper

- The design record is the plan's D15 and row Y2 ([`plans/adopter-surface.md`](../plans/adopter-surface.md)).
- The code: [`src/schedule.rs`](../../src/schedule.rs); the holder election is
  [`src/election.rs`](../../src/election.rs).
- No runbook: a schedule runs inside the application that embeds the node.
