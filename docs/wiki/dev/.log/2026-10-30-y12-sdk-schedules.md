# 2026-10-30 — Y12: an SDK route for running schedules

Adopter-surface row Y12: an SDK agent runs a `[[schedule]]` through the gateway, `GET /gateway/schedules/run` and
`POST /gateway/schedules/ack` (`src/agent/http/schedules.rs`, scope `schedule:run`), and Y6's Eve loader loads
`schedules/` through it (`runEveSchedules`, `cronAsPeriod` in `mycelium-ts/src/eve.ts`).

**Durable knowledge.**

- **The gateway's node stands in the band; the client supplies the work.** The stream runs the same loop as
  `GossipAgent::run_schedule` — `schedule::spawn_schedule`, which takes a `TaskCtx`, so the node's running-schedules
  table moved onto `TaskCtx` — with work that hands the run to the client and waits for its answer. The stream owns the
  loop's handle: closing it ends the loop and withdraws the candidacy. A schedule's work is typed internally
  (`RunFailure`): `Failed` is the work's failure, `Unacknowledged` a run handed out and never answered.
- **A run the client never acknowledged is not a run.** It is traced `sched.run` / `unacknowledged` (a new reason, the
  rule's revision unchanged), its slot is not written to `sched/{id}/last/{node}`, and the node withdraws its holding,
  so the next holder decides the slot as not yet run — on time, or by `sched.missed` with its reason. A client that ran
  the work and lost its answer can run the slot twice; the key `sched/{id}/{slot}` makes that one effect only at a
  deduplicating destination, as for two holders under a partition.
- **The holder band is a capability on the gateway's own node.** The client puts the node in the band by advertising
  the `holders` capability through the gateway; an Eve agent's unit already advertises `eve/<agent>`, the default band
  `runEveSchedules` uses.
- **An Eve cron is read only when it is a period and a phase** in UTC (Eve's own clock: Vercel evaluates crons in UTC):
  every N minutes or hours dividing the hour or the day, hourly, daily or weekly at a time. A weekly phase is counted
  from the epoch's Thursday. Anything else is refused by name, as is a nested Eve schedule (a fleet schedule id is one
  key segment).

**Not built, stated in the plan's Y12 row:** a cron that is not a period and a phase; a band other than a capability
on the gateway's node; a recording of a client's schedule; exactly-once runs; an Eve schedule's `to` and a Markdown
schedule's agent turn; a schedule class in `mycelium test --faults`.
