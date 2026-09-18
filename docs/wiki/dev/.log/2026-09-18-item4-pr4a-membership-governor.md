## [2026-09-18] ingest | item 4 PR 4a — the membership governor through the contract, and a fifth class

Up: [dev](../dev.md) · record `docs/design/adaptive-stability.md` §2 (amended), §3, §9 · code
`src/agent/membership_governor.rs`, `src/agent/control_profile.rs`, `src/control.rs`.

### What landed

The first shipped governor wired to the contract. `classify` maps a membership decision to an action class;
`converge` consults the confidence predicate per pass on the real `ViewConfidence`, observes settling against
the group's membership, and keeps the cooldown as spacing with its meaning unchanged. The node's profile is an
atomic on the task context, settable through `GossipAgent::set_control_profile`, default `Legacy` — so
production behaviour is unchanged until an operator opts in, and `Observe` counts what enforcement would have
held.

### Three things worth carrying forward

1. **The four classes were one short, and the governor said so.** Its own primary action — a join below a
   declared `min` — is not speculation (the bound was declared, the deficit observed) and not rescue from zero
   by name. By *cost* it is exactly rescue: a wrong fill is one extra member, a wrong hold is a group stuck
   below its bound. The rule is about cost, so a fifth class (`DeficitFill`) was added by dated amendment
   rather than the join forced into `SpeculativeScaleUp` and held. Wiring a real governor is what surfaced it;
   the table alone did not.
2. **A drain is not a class.** It is an operator's instruction carried by a fresh intent, not an inference
   from the fleet view, and the predicate judges inferences. Making that `None` rather than "never held" keeps
   the predicate's domain honest.
3. **Settling is observed, not timed.** A pending join or leave clears when the group's membership reflects
   it; the timeout is the fallback that settles it as `unknown`, not the mechanism. The cooldown was already a
   time-based damp; observation-based settling is the thing it lacked.

### Also

The governor's `fastrand::f64`, `Instant` cooldown map and jitter sleep now go through the seams (`govern`,
the monotonic seam, `jitter` + `sleep_ms`), so a replay elects the same; the file's forbidden-site count went
6 → 1, the survivor being `tokio::time::interval` — the timer seam's second arm, not yet built. PR 4 is split
in the ADR's table into 4a (this), 4b (tuning + opacity: local-input governors, spacing and settling only) and
4c (the provisioner reserving against the ledger, and the head published).

### Pages touched

- [history.md](../history.md) — a PR 4a paragraph under the item 4 section.
- The ADR's §2 carries the amendment where the table is; §9 marks 4a landed and names 4b/4c.
