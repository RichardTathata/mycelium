## [2026-09-18] ingest | item 6 PR 7 — the replay corpus and its gate

Up: [dev](../dev.md) · record `docs/design/replay-nondeterminism-inventory.md` §5 · code
`mycelium-core/tests/replay-corpus/scenario-a-wal-snapshot/` (the checked-in bundle), `mycelium-core/src/persistence.rs`
(`record_scenario_a_into_the_corpus`, `the_checked_in_scenario_a_bundle_replays_here_and_matches_a_fresh_recording`),
`mycelium-sim/src/bundle.rs` (`Build::current`).

### What landed

The first entry of a checked-in replay corpus, and the gate that makes it mean something: scenario A's bundle
— recorded on one machine, committed, thirteen effects — must replay on whatever machine runs the suite without
divergence, **and** a fresh recording there must ask for the same effects in the same order. The second half is
the stronger claim, and it is what turns a bundle from a souvenir into a gate: a change in what the code does
fails the suite until someone re-records on purpose and reviews the diff of `choices.trace` — the seams baseline's
discipline applied to effects.

### Three things worth carrying forward

1. **The corpus tests the claim the round-trip test could not.** PR 4's bundle test wrote and read a bundle in
   one process; that proves the format, not the reproduction. A bundle checked in from one machine and replayed
   in CI on another is the claim D14 actually makes — *a bundle is a reproduction artefact* — and the fixed node
   id, the seeded sources and the relative requests are what make it hold. If it ever fails in CI, that failure
   is a nondeterminism the inventory has not named.
2. **Re-recording is a deliberate act with a diff.** The recorder runs only under `MYCELIUM_RECORD_CORPUS=1`
   and writes the same bytes for the same code (checked: two recordings are identical). The trace is text so the
   review is a diff, not a hex dump.
3. **Build identity is recorded only as far as it is honest.** `Build::current` carries the crate version, the
   commit when CI compiled it (`GITHUB_SHA`), the arch and OS — and `unknown` for the compiler, which a crate
   cannot know without a build script. The corpus entry recorded locally says `commit: unknown`; that is the truth
   of where it was made, not a field to fill in by hand.

### Not built

The minimiser (delta-debugging a failing trace) and a replay binary: the recorder and the gate are the tooling
this PR ships. The corpus has one entry; scenarios B and C are pure sweeps with no kernel trace to record.

### Pages touched

- [testing/testing.md](../testing/testing.md) — *The replay corpus*.
- [history.md](../history.md) — the item 6 PR 7 section.
- The inventory's §5 carries a dated note; CHANGELOG; the plan's item 6 sequence marks PR 6–7 done.
