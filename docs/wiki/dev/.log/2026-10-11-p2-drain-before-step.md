## [2026-10-11] ingest | P2 round 2 — drain before step, the fence inside the claim

- Round 2 of #601's review: the proposer's own promise and vote skipped the fence (checked only at the door), and a
  value two or more steps old rested on its commit record. Both closed (decision record §8.2–8.3).
- **The fence inside the claim:** every claim on an electorate group — an acceptor's promise or acceptance, and the
  proposer's own — runs inside a `compute` on the group's fence key (`ConsensusEngine::claim_gated`,
  `TaskCtx::electorate_fences`); a step's promise raises the fence there first. The fence is now set on the step's
  **promise**, not its acceptance.
- **Drain before step:** slots answered on an electorate group are indexed durably (`sys/consensus-slot-group/`,
  self-owned); a step's promise is answered with `StepPrepareAck` carrying the acceptor's report; the step proposer
  runs every reported slot to completion at the old epoch (`DrainPrepare`/`DrainPropose`, commits nothing) before
  proposing the step. Induction in §8.3: every possibly-chosen value is held by a majority of the current or previous
  epoch. Bound: 256 slots per group per member, 4 KiB values — past it `DrainRefused` by name.
- Also: fleet-exclusive slots decided only in the fleet electorate, `leader/{g}` only in `g`; acceptors refuse
  cluster-scoped exclusive slots (incl. `capauthz/`, awards) once a group is marked; refusals member-bound; refused
  certificates not re-verified; the electorate list rescanned on change only; `declare` completes a pending step and
  reports `adopted`. Why the default marking is not a cluster-scoped decision: §8.5.
- Test lore: the electorate meshes are serialised within the test process (`ELECTORATE_MESHES`), readiness is
  write-reachability, and anti-entropy runs every 2 s there — side by side on a loaded host, five-node meshes lost
  gossip long enough to fail rosters that converge in a second alone.
- Pages touched: `dev/architecture/runtime-invariants.md`. Outside the wiki: the decision record §8, threat model §7,
  guide 04, the FAQ, `what-is-proven.md`, `src/lib.rs` (namespace row), CHANGELOG.
