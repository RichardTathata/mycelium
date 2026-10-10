## [2026-10-10] ingest | the adversarial review of #600 (row A)

- Finding 1 (high): a lifecycle record naming another value made the committed entry read live with no window, and
  LWW can keep that pairing for good. Now bounded (the record's window from the entry; 30 s for a release record),
  a COMMIT below the record's ballot is stale whatever its value, and a release refuses over a record for another
  value or below a known decided ballot. Three two-node tests reorder the frames on purpose.
- Also: `release_leadership` async and fsynced; a pre-2.30.0 permanent slot released at the ballot acceptor memory
  holds; collection on its own task, 64 slots a tick, and a gossip-received floor re-appended with `append_sync`
  before it is relied on; the mixed-fleet upgrade note says what a 2.31 learner's re-stamp does to an upgraded fleet.
- Pages touched: `dev/architecture/runtime-invariants.md` (mismatch reading, collection condition),
  `dev/concurrency/lock-order.md` (row 56's hold), `docs/guide/04-consensus.md`, `docs/operations/rbac.md`
  (node-wide `consensus:write` on the election routes), `docs/design/consensus-electorate.md` and
  `docs/philosophy.md` (C1, C2 delivered).
