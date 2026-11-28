## [2026-11-27] ingest | ER2 — a schedule's declaration can change (R6-02, with R6-05)

**What landed.** Row ER2 of `docs/plans/engineering-review-urgent-fixes.md` §4. The writer of
`sched/{id}/last/{node}` (`Runner::tick`'s success arm, `src/schedule.rs`) took the larger of its own entry's slot and
the slot run, whatever period and phase the entry was written under, and stamped the result with the current
declaration. The reader (`fleet_last`) counts only entries of the current period and phase and at most
`MAX_AHEAD_SLOTS` ahead. Reader and writer now share `parse_last`, `counted` and `recorded`: the writer keeps its
entry only when the reader would count it. The success arm reads the bound against `tick.slot` — no new seam draw.

**The durable lesson.** *A writer that keeps a value its reader refuses to count is a loop, not a cache.* Three
triggers had one root: a lengthened period (the entry ~9e8 slots ahead, the slot re-run every check interval), a
phase change (the old slot one ahead under the new phase: a slot skipped), and an own entry left past the bound by a
clock since corrected (out of the count and re-written after every run). The parent plan thought shortening and
phase-only changes safe; only shortening is. When a reader filters, the writer that feeds it must filter the same way
— share the predicate, do not restate it.

**A clock stepped back in place does not reach the loop.** 64 slots is always more than three lease refreshes, so a
node whose clock steps back that far reads its own candidacy as future-stamped (`CapEntry::is_fresh`'s symmetric
window; the HLC keeps its stamps ahead) and holds nothing until the clock catches up — silent, not re-running. The
re-run path is the entry outliving the clock: the node runs ahead, comes back corrected with a fresh HLC, and its
entry returns from a peer or its store. The test `a_clock_stepped_back_past_the_bound_runs_each_slot_once` builds
that path; the design's in-place step was not reachable.

**R6-05 was a sentence.** A failed late or gap run is run again when no later run of its tick completes (nothing
advanced the record); its trace said *not retried*. The trace and the module doc now say what happens. `sched.run` and
`sched.missed` are at revision 2 (D14b). The replay corpus pins `rule_revision` in its projection
(`tests/schedule_replay.rs`), so the unchanged corpus failed on the revision alone, after consuming every recorded
choice; re-recorded, only `decisions.jsonl`'s six `rule_revision` fields changed — `choices.trace` byte-identical,
so no draw was added.

**Seen failing first:** see the commit message. Upgrade note `deprecations.md` §TBD-ER2.
