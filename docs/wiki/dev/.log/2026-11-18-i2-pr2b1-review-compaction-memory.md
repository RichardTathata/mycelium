## [2026-11-18] ingest | CA succession, PR 2b-1: what #812's review found in the compaction and the commitment (I2)

- **A record is removed for what it no longer carries, not for being dead.** An `Anchor` record rebuilds three things
  at a start: the peer's entry in `anchors_seen` (an entry with no anchor trusts nothing; no entry is legacy), the
  kept certificate, and the journaled set. The first compaction removed every dead record, and so the entry: after a
  restart a fail-closed peer was legacy again (F1). Now the first dead record of a peer with no kept certificate on
  disk stays.
- **"Never believes one durable that is not" has to be argued against the settle.** An append is handed to the
  writer and settled later; a rewrite between the two dropped the frame and the settle then called it journaled (F2).
  The choice now keeps an in-flight record, and looks, takes off, looks again.
- **A counter means what its definition says.** `ca_commitment_conflict` is two commitments the same CA's key signed:
  an unverifiable document is `ca_commitment_unverified`; a commitment by another CA is neither (and whether it is
  pinned once a pin is spent is an open decision, plan row I2).
- **A scan over a prefix any member can write is bounded before it hashes**: size, count per scan, a gauge for what
  was left, and noted sets that are never cleared.
- Pages touched: `dev/architecture/runtime-invariants.md` (§Persistence), `dev/owner-signed-records.md` (the
  `sys/ca-commit/` row: **Outside: no decision**, with its sites).
