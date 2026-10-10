## [2026-10-10] ingest | the adversarial re-review of #600 (row A)

- Finding 1: the bounded mismatch reading (the first review's fix) could not tell a newer commit under an older
  record from the reverse, and the newer-commit direction is every handover's normal state (a learner applies
  `committed` + `decided` from the COMMIT signal; the record is its own key). Rule now: a lifecycle record below
  the decided ballot says nothing about the entry; the committer writes the record before emitting the COMMIT. A
  five-node test withholds B's record from the learners and shows a third proposer refused.
- Also: permanent commits always tombstone the record key; a permanent leader re-committed higher releases at that
  ballot; acceptor memory names a release ballot only with no `decided` key; the gateway's release distinguishes
  `500 release_unrecorded`; `persist_floor` appends after `record_decided` always; collection rotates its start;
  `record_ballot` carries the floor tripwire.
- Pages touched: `dev/architecture/runtime-invariants.md` (the reading rule, release refusals), `docs/guide/04-consensus.md`.
