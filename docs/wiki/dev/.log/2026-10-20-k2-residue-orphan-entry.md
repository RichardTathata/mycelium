## [2026-10-20] ingest | K2 residue: an entry outliving its collected heartbeat leaked

- `residue_is_collected_and_a_decision_does_not_read_it` failed intermittently (#654's CI; ~1 in 900 runs under 32
  parallel processes). Not a test race: `collect_residue` (`src/agent/fleet_history.rs`) dated a non-peer by its
  heartbeat and skipped a node with none, so an entry reaching the collector after the heartbeat was collected leaked
  for ever (store and scope roster). The test hit it when the node's own collector tick took a planted heartbeat
  before its entry was planted; production hits it through anti-entropy or gossip order.
- Fix: an entry with no heartbeat beside it is dated by its own times, collected once every time is past its window
  + staleness + clock drift; an unreadable one is kept. Fail-first
  `an_entry_outliving_its_collected_heartbeat_is_collected`.

Lesson: a collector that keys its decision on one record of a group must still handle the group's other records when
that one is gone — collection is not atomic across keys, and arrival order is not the write order.

Pages: `dev/dev.md` (K2 paragraph). Repo: `CHANGELOG.md`, `docs/plans/post-360-hardening.md` §3.6.
