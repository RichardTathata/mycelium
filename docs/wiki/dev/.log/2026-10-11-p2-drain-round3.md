## [2026-10-11] ingest | P2 round 3 — committed slots drained, no permanent fence

- Round 3 of #601's review: a committed slot was never drained (the drain ran with the commit shortcuts), and the
  drain's bounds were checked after the promises fenced the group, with no way to lower a fence.
- Now (`src/consensus.rs`, decision record §8.2–8.4): the drain takes no shortcut on a visible commit (`ProposeMode::
  Drain` skips `superseded_by_live` and `DecidedOtherwise`); the report is taken **before** the promise and an
  acceptor that cannot deliver it answers `StepRefused` (no fence); only open slots count (finished ones are collected
  from the index); values of any size up to 4 MiB a report, a digest-only one fetched from its commit record; a
  refused step sends `StepAbort` and members that promised without accepting mark the promise abandoned
  (`ABANDONED_PROMISE`), releasing the fence; the index is per `(group, slot)`; a member that accepted a genesis
  refuses untagged proposals on the group until adopted; `claim_gated` decides in the `compute` and claims after it
  (an in-flight count orders ordinary claims against a step's raise); cross-group proposals never decide an exclusive
  slot; the drain's set-aside uses the proposer's own reading.
- Pages touched: `dev/concurrency/lock-order.md` (row 57, renumbered from 56 — #600 holds 56). Outside the wiki: the
  decision record §8, threat model §7 (drain proposals answered from any member), `what-is-proven.md`, CHANGELOG.
- Test lore (round 3's flakes, root-caused): the electorate meshes stayed **sparse** — a node that dials peers not
  yet listening backs off, and the peer table did not densify for a minute; a group-scoped signal is forwarded to the
  group's *known* members, so a member nobody reached directly missed whole rounds (genesis timeouts, lost
  `ProposeIn`s). `electorate_mesh` now has each node dial only the nodes started before it, waits for full direct
  peering plus write-reachability, and keeps `reconnect_backoff_secs < health_check_interval_secs − 2` (an interval
  of 2 s with the 5 s default backoff evicted and redialled peers in a loop). 12 × 13 multi-node runs: 156/156.
