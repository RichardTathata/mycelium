## [2026-10-07] ingest | S5: why a checkpoint blob is missing stays distinguishable (#542)

- `mycelium-reason` 0.7.0: `BlobMiss` (`NotFound`/`Unavailable`/`Corrupt`), the route's 404/503/502 with a body
  reason, `LocalRead::Damaged` for a bad hash or an unreadable file, the stock server's `DAMAGED_REPLY`, `put`
  repairing a damaged copy. Checkpointer 0.3.0: reasons read from the body; `retriable` only when all transient.
- Three review rounds: the first found `Corrupt` overclaiming (one bad provider beside an honest one lacking the
  blob made a spreading blob permanent), damage at rest invisible, a proxy's 502 read as corrupt; the second, a
  truncated remote copy read as a miss and damage unrepairable; the third, EIO read as absence and a re-hash per
  dedup put.
- Pages: `dev/companions/companions.md` (the reason companion's paragraph).
