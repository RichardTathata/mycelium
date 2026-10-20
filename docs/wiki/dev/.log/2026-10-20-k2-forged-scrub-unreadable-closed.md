## [2026-10-20] ingest | K2: the owner scrubs forged times; unreadable tally entries fail closed again

- #656's second review, round 2: a forged `t: [now, u64::MAX]` in a *live* member's entry held its counter
  `Indeterminate` until the owner next published (up to the window). The owner's `withdraw_aged_entries` now rewrites
  its own entries without forged times under the tally lock, so the bound is one heartbeat tick plus propagation.
  Honest-looking forged times still inflate a count (unsigned entries, K2's stated limit).
- Unreadable entries are no longer collected at once: they go with their node's heartbeat (or, with none, once nothing
  of the node is live). A count that cannot be read is not zero, and an older collector must not erase a newer
  node's entries during a format change.
- The heartbeat's 24 h linger costs about one extra KV read per tally decision per recently departed node — stated.
- Four mutations that survived round 1 are now killed by tests (publish drops forged times, `aged` ignores them, the
  reader's future-heartbeat filter on a verified member, `!live` in the heartbeat rule).

Lesson: "collect what cannot be read" and "a count that cannot be read is not zero" pull opposite ways; the second
wins whenever a reader may be older than the writer.

Pages: none beyond `dev/dev.md` (unchanged this round). Repo: `src/agent/fleet_history.rs`, `CHANGELOG.md`,
`docs/guide/deprecations.md` §56, `docs/guide/20-authorising-actions.md`, `src/lib.rs`,
`docs/plans/post-360-hardening.md` §3.6.
