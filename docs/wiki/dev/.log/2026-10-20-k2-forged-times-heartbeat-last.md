## [2026-10-20] ingest | K2: forged tally times date nothing; a heartbeat is collected last

- #656's second independent review, four items, each closed with a test seen failing first (or a mutation it kills):
  - a time past now + `max_clock_drift_ms` is forged (tally keys are unsigned): reader reads the entry as unreadable,
    owner drops/ignores it (`publish_permit`, `aged`), collector dates entries by honest times only — a forged-only
    entry goes at once; a heartbeat from the future is no heartbeat;
  - "exactly" qualified: reader and collector agree when the writer's window is the counter's;
  - `collect_residue` takes each entry by its own times and the heartbeat last, past `MAX_FLEET_WINDOW_MS` + bounds —
    a collector cannot see windows held only elsewhere. Cost: a departed node's heartbeat stays up to 24 h + bounds;
  - two reader tests that survived mutation now kill it (`.unwrap_or(true)`, horizon vs window).

Lessons:
- An unsigned time is an input, not a clock: every reader of one needs a "from the future" cut, or one forged value
  outlives every window.
- A collector that derives a bound from the keys it holds is wrong whenever another node holds a key it does not; use
  a bound that does not depend on what is held (here, the cap on any window).
- A test that removes a peer by hand leaves the connection standing; restarting that node at once is not re-admitted.
  Give such a test the dwell its original timing implied, stated as a condition, not a sleep.

Pages: `dev/dev.md` (K2 paragraph). Repo: `src/agent/fleet_history.rs`, `docs/guide/20-authorising-actions.md`,
`docs/guide/deprecations.md` §56, `CHANGELOG.md`, `docs/plans/post-360-hardening.md` §3.6, `src/lib.rs`.
