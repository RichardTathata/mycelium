## [2026-11-27] ingest | ER2 — a schedule's declaration can change (R6-02, with R6-05)

**What landed.** Row ER2 of `docs/plans/engineering-review-urgent-fixes.md` §4. The writer of
`sched/{id}/last/{node}` (`Runner::tick`'s success arm, `src/schedule.rs`) took the larger of its own entry's slot and
the slot run, whatever period and phase the entry was written under, and stamped the result with the current
declaration. The reader (`fleet_last`) counts only entries of the current period and phase and at most
`MAX_AHEAD_SLOTS` ahead. Reader and writer now share `parse_last`, `counted` and `recorded`: the writer keeps its
entry only when the reader would count it. The success arm reads the bound against `tick.slot` — no new seam draw.

**The durable lesson** (now on [runtime-invariants](../architecture/runtime-invariants.md), *A writer must not keep
what its reader refuses*). *A writer that keeps a value its reader refuses to count is a loop, not a cache.* Two
triggers had one root: a lengthened period (the entry ~9e8 slots ahead, the slot re-run every check interval) and a
phase change (the old slot one ahead under the new phase: a slot skipped). A far-ahead value another member writes to
the node's own key is the third, and the only form of the design's clock case ER2 fixes. The parent plan thought
shortening and phase-only changes safe; only shortening is. A phase change is fixed for slot numbers only: the run's
key `sched/{id}/{slot}` names no phase.

**The clock case is not fixed, and review round 1 showed why** (2026-11-28). The first version of the clock test
seeded the entry with `kv().set`, which re-stamps it with the node's current HLC, so it tested a path no node takes. A
node that ran with its clock ahead gets its entry back **with the original stamp** — from its store (`hlc.observe` of
the replayed maximum, `src/agent/lifecycle.rs`) or from a peer. Within `max_clock_drift_ms` its clock is carried up to
the stamp and its own advertisements read as from the future (`CapEntry::is_fresh`): silent until the clock catches
up. Past the drift bound the entry holds its key under LWW (`what-is-proven.md#far-future-stamp-holds-its-key`) and the
slot re-runs each check interval — a Layer I limit. **A test that seeds state through a path that rewrites its
metadata (a stamp) tests a state no real path produces:** seed through the path the state really comes back by. The
same round found the trace claiming a replacement after a write that lost LWW; the writer now reads its
local-application rung (`ops::kv_set_reporting`) and says *superseded*.

**R6-05 was a sentence.** A failed late or gap run is run again when no later run of its tick completes (nothing
advanced the record); its trace said *not retried*. The trace and the module doc now say what happens. `sched.run` and
`sched.missed` are at revision 2 (D14b). The replay corpus pins `rule_revision` in its projection
(`tests/schedule_replay.rs`), so the unchanged corpus failed on the revision alone, after consuming every recorded
choice; re-recorded, only `decisions.jsonl`'s six `rule_revision` fields changed — `choices.trace` byte-identical,
so no draw was added.

**Seen failing first:** see the commit message. Upgrade note `deprecations.md` §133.
