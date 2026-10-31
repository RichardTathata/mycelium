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

The section is `[[schedule]]` ([unit-file § schedule](../reference/unit-file.md#schedule)); the work is supplied by
the embedding application or, through the gateway, by an SDK agent.

- [`run_schedule`](../../src/schedule.rs) — `GossipAgent::run_schedule(&decl, work)` takes part in electing the
  holder and, while holding, calls `work` once per run; `work` returns `Err` to have the slot retried at the next
  tick.
- [`ScheduledRun`](../../src/schedule.rs) — one run: the slot, why it runs (on time, late, for a gap), and
  `operation_id`, the key an effect it causes carries.
- [`decide`](../../src/schedule.rs) — one tick's decision, pure.
- An SDK agent: `GET /gateway/schedules/run?toml=<one [[schedule]]>` ([`src/agent/http/schedules.rs`](../../src/agent/http/schedules.rs),
  scope `schedule:run`) makes the node the client is connected to a candidate in the band while the stream is open,
  and hands the client each run it decides as a `run` event — the slot, the kind and `operation_id`; the client
  answers each with `POST /gateway/schedules/ack` within one period. The verbs are `scheduleRuns` / `scheduleAck` /
  `runSchedule` in [`mycelium-ts`](../../mycelium-ts/README.md) and `schedule_runs` / `schedule_ack` in
  [`mycelium-py`](../../mycelium-py/README.md); the node must match `holders` (advertise the capability on it).
  An Eve agent's `schedules/` loads through Y6's loader and runs this way (`runEveSchedules`, in
  [`mycelium-ts/src/eve.ts`](../../mycelium-ts/src/eve.ts)) when its cron is a period and a phase.
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
`a_host_outside_the_holder_band_never_runs`; through the gateway, in
[`src/agent/http/schedule_run_tests.rs`](../../src/agent/http/schedule_run_tests.rs):
`a_client_receives_its_schedules_runs_and_an_acknowledged_run_records_its_slot`,
`a_run_never_acknowledged_is_traced_and_its_slot_is_not_recorded`,
`slots_missed_while_no_client_held_are_a_decision_with_its_reason` and
`two_clients_under_a_partition_are_handed_one_key_per_slot`, and each SDK's live suite
(`mycelium-ts/tests/live/schedules.test.ts`, `mycelium-py/tests/live/test_schedules.py`); `wire-check`'s refusal of a schedule no unit can hold is
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
- **A client's schedule is its principal's.** The gateway runs it under `{id}~{principal}` — the principal as the
  gateway derives it from the caller's credential, never from the request, percent-encoded (`schedule::client_scoped_id`)
  — so its ring, advertisements, `sched/` entries and trace are its own. A client cannot reach an application's
  `run_schedule` of the same id (an application id cannot hold `~`), nor another principal's. **Clients that share a
  principal share a namespace**: on an open gateway every client is `anonymous`, and every client presenting one bearer
  or one scoped token is one principal. The client sees its own id in each run's `schedule`; the run's `operation_id`
  is the scoped key `sched/{id}~{principal}/{slot}`, so two principals' runs of one slot are two effects.
- **Through the gateway, a run the client never acknowledged is not a run.** The stream closed or the client never
  answered: the run is traced `unacknowledged` (or `work_overran` past its period), the slot is not recorded, and
  the node withdraws its holding, so the next holder decides the slot as one not yet run — on time, or as a missed
  run under the policy. A client that ran the work and lost its answer can run the slot twice: the key makes that
  one effect at a destination that deduplicates, as for two holders. A client that stays connected and stalls — its
  run overruns the period — makes its node **stand down**: candidacy and holding withdrawn for ten periods (at least
  twice the warm-up, `stood_down`; `schedule::stand_down_ms`), so another node's client takes the slots. A stream whose node runs nothing says
  why in a `status` event (`not_eligible` for a node outside `holders`); `onStatus` / `on_status` surface it. A unit
  cannot carry a schedule
  (`POST /gateway/units/declare` refuses one, 422, naming the route).
- Not built, stated in the plan's Y2 and Y12 rows: a cron language beyond a period and a phase (an Eve cron that is
  not one is refused by name), a fleet-wide calendar, a stem that runs a schedule (it refuses one by name), a
  schedule class in `mycelium test --faults`, a recording of a client's schedule (the node's loop goes through the
  seams; the client's work is outside them), and a holder band other than a capability on the gateway's own node.
- A recording replays it: the loop's clock, tick and deadline go through the seams, and the checked-in recording
  `tests/replay-corpus/y2-schedule-run/` (one node holding `close`, deciding a missed run under `run_once`, a run
  that overruns its deadline, then the gap's run and the on-time run) is replayed decision for decision in another
  process by `tests/schedule_replay.rs`, run in CI. `run_late`, `skip` and a failed run are covered by unit tests,
  not a recording.

## Go deeper

- The design record is the plan's D15 and row Y2 ([`plans/adopter-surface.md`](../plans/adopter-surface.md)).
- The code: [`src/schedule.rs`](../../src/schedule.rs); the holder election is
  [`src/election.rs`](../../src/election.rs).
- No runbook: a schedule runs inside the application that embeds the node, or in an SDK agent through its gateway.
